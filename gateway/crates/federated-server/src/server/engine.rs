use crate::config::TelemetryConfig;

use super::{gateway::GatewayWatcher, ServerState};
use axum::{
    body::Body,
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use engine::BatchRequest;
use futures_util::{
    future::{self, BoxFuture},
    stream,
};
use gateway_core::{encode_stream_response, RequestContext};
use gateway_v2::streaming::StreamingFormat;
use http::{header, HeaderMap};
use response::BatchResponse;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;
use ulid::Ulid;

mod context;
mod response;

pub(super) async fn get(
    Query(request): Query<engine::QueryParamRequest>,
    headers: HeaderMap,
    State(state): State<ServerState>,
) -> impl IntoResponse {
    let request = engine::BatchRequest::Single(request.into());
    traced(headers, request, state.gateway().clone(), state.telemetry_config()).await
}

pub(super) async fn post(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<engine::BatchRequest>,
) -> impl IntoResponse {
    traced(headers, request, state.gateway().clone(), state.telemetry_config()).await
}

#[cfg(feature = "lambda")]
async fn traced(
    headers: HeaderMap,
    request: BatchRequest,
    gateway: GatewayWatcher,
    telemetry_config: Option<&TelemetryConfig>,
) -> http::Response<Body> {
    // lambda has no global tracing, so we initialize it here and force flush before responding

    use grafbase_tracing::otel::opentelemetry::trace::TracerProvider;
    use grafbase_tracing::otel::tracing_subscriber::layer::SubscriberExt;
    use grafbase_tracing::otel::tracing_subscriber::EnvFilter;
    use grafbase_tracing::otel::{self, opentelemetry_sdk::runtime::Tokio};
    use grafbase_tracing::otel::{tracing_opentelemetry, tracing_subscriber};
    use tracing_futures::WithSubscriber;

    match telemetry_config {
        Some(config) => {
            let provider = otel::layer::new_provider(&config.service_name, &config.tracing, Tokio).unwrap();

            let tracer = provider.tracer("lambda-otel");

            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .with(tracing_subscriber::fmt::layer().with_ansi(false))
                .with(EnvFilter::new(&config.tracing.filter));

            let response = handle(headers, request, gateway).with_subscriber(subscriber).await;

            for result in provider.force_flush() {
                if let Err(e) = result {
                    println!("failed to flush traces: {e}");
                }
            }

            response
        }
        None => handle(headers, request, gateway).await,
    }
}

#[cfg(not(feature = "lambda"))]
async fn traced(
    headers: HeaderMap,
    request: BatchRequest,
    gateway: GatewayWatcher,
    _: Option<&TelemetryConfig>,
) -> http::Response<Body> {
    // we do global tracing on non-lambda context.

    handle(headers, request, gateway).await
}

async fn handle(headers: HeaderMap, request: BatchRequest, gateway: GatewayWatcher) -> http::Response<Body> {
    let Some(gateway) = gateway.borrow().clone() else {
        return Json(json!({
            "errors": [{"message": "there are no subgraphs registered currently"}]
        }))
        .into_response();
    };

    let streaming_format = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .and_then(StreamingFormat::from_accept_header);

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

    let ctx = context::RequestContext {
        ray_id: Ulid::new().to_string(),
        headers,
        wait_until_sender: sender,
    };

    let ray_id = ctx.ray_id.clone();

    match streaming_format {
        Some(format) if request.has_multiple_operations() => {
            let payload_stream = stream::once(async {
                let message = "Batch requests cannot be combined with streaming response formats at present";
                engine_v2::Response::error(message, [])
            });

            let (headers, stream) = encode_stream_response(ray_id.clone(), payload_stream, format).await;
            return (headers, Body::from_stream(stream)).into_response();
        }
        _ => (),
    }

    let Some(session) = gateway.authorize(ctx.headers()).await else {
        match (request, streaming_format) {
            (BatchRequest::Single(_), None) => {
                let response = gateway_v2::Response::unauthorized();

                return (response.status, response.headers, response.bytes).into_response();
            }
            (BatchRequest::Single(_), Some(format)) => {
                let (headers, stream) = encode_stream_response(
                    ray_id,
                    stream::once(async { engine_v2::Response::error("Unauthorized", []) }),
                    format,
                )
                .await;

                return (headers, axum::body::Body::from_stream(stream)).into_response();
            }
            (BatchRequest::Batch(requests), _) => {
                let batch_response = BatchResponse::Batch(
                    std::iter::repeat_with(gateway_v2::Response::unauthorized)
                        .take(requests.len())
                        .collect(),
                );

                return batch_response.into_response();
            }
        }
    };

    let response = match (request, streaming_format) {
        (BatchRequest::Single(mut request), None) => {
            request.ray_id = ctx.ray_id.clone();
            BatchResponse::Single(session.execute(&ctx, request).await)
        }
        (BatchRequest::Single(mut request), Some(streaming_format)) => {
            request.ray_id = ctx.ray_id.clone();

            let (headers, stream) =
                encode_stream_response(ray_id, session.execute_stream(request), streaming_format).await;

            tokio::spawn(wait(receiver));

            return (headers, axum::body::Body::from_stream(stream)).into_response();
        }
        (BatchRequest::Batch(requests), None) => {
            let mut responses = Vec::with_capacity(requests.len());
            for mut request in requests {
                request.ray_id = ctx.ray_id.clone();
                responses.push(session.clone().execute(&ctx, request).await)
            }
            BatchResponse::Batch(responses)
        }
        (BatchRequest::Batch(_), Some(_)) => {
            unreachable!("should have been dealt with above")
        }
    };

    tokio::spawn(wait(receiver));

    response.into_response()
}

async fn wait(mut receiver: UnboundedReceiver<BoxFuture<'static, ()>>) {
    // Wait simultaneously on everything immediately accessible
    future::join_all(std::iter::from_fn(|| receiver.try_recv().ok())).await;

    // Wait sequentially on the rest
    while let Some(fut) = receiver.recv().await {
        fut.await;
    }
}
