use ::runtime::{
    auth::AccessToken,
    hooks::Hooks,
    operation_cache::{OperationCache, OperationCacheFactory},
    rate_limiting::RateLimitKey,
};
use async_runtime::stream::StreamExt as _;
use engine::{BatchRequest, Request};
use engine_parser::types::OperationType;
use futures::{channel::mpsc, FutureExt, StreamExt};
use futures_util::{SinkExt, Stream};
use gateway_core::StreamingFormat;
use gateway_v2_auth::AuthService;
use grafbase_telemetry::{
    gql_response_status::GraphqlResponseStatus,
    grafbase_client::Client,
    metrics::{GraphqlOperationMetrics, GraphqlRequestMetricsAttributes, OperationMetricsAttributes},
    span::{gql::GqlRequestSpan, GqlRecorderSpanExt, GRAFBASE_TARGET},
};
use headers::HeaderMapExt;
use retry_budget::RetryBudgets;
use schema::Schema;
use std::{borrow::Cow, sync::Arc};
use tracing::Instrument;
use trusted_documents::PreparedOperationDocument;
use web_time::Instant;

use crate::{
    execution::{ExecutableOperation, PreExecutionContext},
    http_response::HttpGraphqlResponse,
    operation::{Operation, PreparedOperation, Variables},
    response::{ErrorCode, GraphqlError, Response},
    websocket,
};

mod cache;
mod retry_budget;
mod runtime;
mod trusted_documents;

pub use runtime::Runtime;

pub(crate) struct SchemaVersion(Vec<u8>);

impl std::ops::Deref for SchemaVersion {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub struct Engine<R: Runtime> {
    // We use an Arc for the schema to have a self-contained response which may still
    // needs access to the schema strings
    pub(crate) schema: Arc<Schema>,
    pub(crate) schema_version: SchemaVersion,
    pub(crate) runtime: R,
    operation_metrics: GraphqlOperationMetrics,
    auth: AuthService,
    retry_budgets: RetryBudgets,
    operation_cache: <R::OperationCacheFactory as OperationCacheFactory>::Cache<Arc<PreparedOperation>>,
}

impl<R: Runtime> Engine<R> {
    /// schema_version is used in operation cache key which ensures we only retrieve cached
    /// operation for the same schema version. If none is provided, a random one is generated.
    pub async fn new(schema: Arc<Schema>, schema_version: Option<&[u8]>, runtime: R) -> Self {
        let auth = gateway_v2_auth::AuthService::new_v2(
            schema.settings.auth_config.clone().unwrap_or_default(),
            runtime.kv().clone(),
        );

        Self {
            schema_version: SchemaVersion({
                let mut out = Vec::new();
                match schema_version {
                    Some(version) => {
                        out.push(0x00);
                        out.extend_from_slice(version);
                    }
                    None => {
                        out.push(0x01);
                        out.extend_from_slice(&ulid::Ulid::new().to_bytes());
                    }
                }
                out
            }),
            auth,
            retry_budgets: RetryBudgets::build(&schema),
            operation_metrics: GraphqlOperationMetrics::build(runtime.meter()),
            operation_cache: runtime.operation_cache_factory().create().await,
            schema,
            runtime,
        }
    }

    pub async fn execute(
        self: &Arc<Self>,
        headers: http::HeaderMap,
        batch_request: BatchRequest,
    ) -> HttpGraphqlResponse {
        use futures_util::{pin_mut, select, FutureExt};

        let format = headers.typed_get::<StreamingFormat>();
        let request_context = match self.create_request_context(headers).await {
            Ok(context) => context,
            Err(response) => return HttpGraphqlResponse::build(response, format),
        };

        if let Err(err) = self.runtime.rate_limiter().limit(&RateLimitKey::Global).await {
            return HttpGraphqlResponse::build(
                Response::pre_execution_error(GraphqlError::new(err.to_string(), ErrorCode::RateLimited)),
                format,
            );
        }

        let mut timeout = match format {
            Some(_) => {
                // Streaming requests are subscriptions so shouldn't timeout
                std::future::pending().boxed()
            }
            None => async move {
                self.runtime.sleep(self.schema.settings.timeout).await;
                HttpGraphqlResponse::build(
                    Response::execution_error(GraphqlError::new("Gateway timeout", ErrorCode::GatewayTimeout)),
                    format,
                )
            }
            .boxed(),
        }
        .fuse();

        let execution = self.execute_maybe_batch(request_context, batch_request).fuse();
        pin_mut!(execution);

        select!(
           response = timeout => response,
           response = execution => response
        )
    }

    pub async fn create_session(self: &Arc<Self>, headers: http::HeaderMap) -> Result<Session<R>, Cow<'static, str>> {
        if let Err(err) = self.runtime.rate_limiter().limit(&RateLimitKey::Global).await {
            return Err(
                Response::pre_execution_error(GraphqlError::new(err.to_string(), ErrorCode::RateLimited))
                    .first_error_message()
                    .unwrap_or("Internal server error".into()),
            );
        }

        let request_context = match self.create_request_context(headers).await {
            Ok(context) => context,
            Err(response) => return Err(response.first_error_message().unwrap_or("Internal server error".into())),
        };

        Ok(Session {
            engine: Arc::clone(self),
            request_context: Arc::new(request_context),
        })
    }

    async fn create_request_context(
        &self,
        headers: http::HeaderMap,
    ) -> Result<RequestContext<<R::Hooks as Hooks>::Context>, Response> {
        let client = Client::extract_from(&headers);
        let streaming_format = headers.typed_get::<StreamingFormat>();

        let (hooks_context, headers) = self
            .runtime
            .hooks()
            .on_gateway_request(headers)
            .await
            .map_err(Response::pre_execution_error)?;

        if let Some(access_token) = self.auth.authenticate(&headers).await {
            Ok(RequestContext {
                headers,
                streaming_format,
                client,
                access_token,
                hooks_context,
            })
        } else {
            Err(Response::pre_execution_error(GraphqlError::new(
                "Unauthenticated",
                ErrorCode::Unauthenticated,
            )))
        }
    }

    async fn execute_maybe_batch(
        self: &Arc<Self>,
        request_context: RequestContext<<R::Hooks as Hooks>::Context>,
        batch_request: BatchRequest,
    ) -> HttpGraphqlResponse {
        match batch_request {
            BatchRequest::Single(request) => {
                if let Some(streaming_format) = request_context.streaming_format {
                    convert_stream_to_http_response(
                        streaming_format,
                        self.execute_stream(Arc::new(request_context), request),
                    )
                    .await
                } else {
                    self.execute_single(&request_context, request).await
                }
            }
            BatchRequest::Batch(requests) => {
                if request_context.streaming_format.is_some() {
                    return HttpGraphqlResponse::bad_request_error(
                        "batch requests can't use multipart or event-stream responses",
                    );
                }
                HttpGraphqlResponse::from_batch(
                    futures_util::stream::iter(requests.into_iter())
                        .then(|request| self.execute_single(&request_context, request))
                        .collect::<Vec<_>>()
                        .await,
                )
            }
        }
    }

    async fn execute_single(
        &self,
        request_context: &RequestContext<<R::Hooks as Hooks>::Context>,
        request: Request,
    ) -> HttpGraphqlResponse {
        let start = Instant::now();
        let span = GqlRequestSpan::create();
        async {
            let ctx = PreExecutionContext::new(self, request_context);
            let (operation_metrics_attributes, response) = ctx.execute_single(request).await;
            let status = response.status();

            let elapsed = start.elapsed();

            if let Some(operation_metrics_attributes) = operation_metrics_attributes {
                tracing::Span::current().record_gql_request((&operation_metrics_attributes).into());

                self.operation_metrics.record(
                    GraphqlRequestMetricsAttributes {
                        operation: operation_metrics_attributes,
                        status,
                        cache_status: None,
                        client: request_context.client.clone(),
                    },
                    elapsed,
                );
            }

            if status.is_success() {
                tracing::Span::current().record_gql_status(status);
                tracing::debug!(target: GRAFBASE_TARGET, "gateway request")
            } else {
                let message = response
                    .first_error_message()
                    .map(|s| s.into_owned())
                    .unwrap_or_else(|| String::from("gateway error"));

                tracing::Span::current().record_gql_status(status);
                tracing::debug!(target: GRAFBASE_TARGET, "{message}")
            }

            HttpGraphqlResponse::build(response, None)
        }
        .instrument(span)
        .await
    }

    fn execute_stream(
        self: &Arc<Self>,
        request_context: Arc<RequestContext<<R::Hooks as Hooks>::Context>>,
        request: Request,
    ) -> impl Stream<Item = Response> + Send + 'static {
        let start = Instant::now();
        let engine = Arc::clone(self);
        let (sender, receiver) = mpsc::channel(2);

        let span = GqlRequestSpan::create();
        let span_clone = span.clone();
        receiver.join(
            async move {
                let ctx = PreExecutionContext::new(&engine, &request_context);
                let (operation_metrics_attributes, status) = ctx.execute_stream(request, sender).await;
                let elapsed = start.elapsed();

                if let Some(operation_metrics_attributes) = operation_metrics_attributes {
                    tracing::Span::current().record_gql_request((&operation_metrics_attributes).into());

                    engine.operation_metrics.record(
                        GraphqlRequestMetricsAttributes {
                            operation: operation_metrics_attributes,
                            status,
                            cache_status: None,
                            client: request_context.client.clone(),
                        },
                        elapsed,
                    );
                }

                span.record_gql_status(status);

                if status.is_success() {
                    tracing::debug!(target: GRAFBASE_TARGET, "gateway request")
                } else {
                    tracing::debug!(target: GRAFBASE_TARGET, "gateway error")
                }
            }
            .instrument(span_clone),
        )
    }
}

async fn convert_stream_to_http_response(
    streaming_format: StreamingFormat,
    stream: impl Stream<Item = Response> + Send + 'static,
) -> HttpGraphqlResponse {
    let mut stream = Box::pin(stream);
    let Some(first_response) = stream.next().await else {
        return HttpGraphqlResponse::internal_server_error("Empty stream");
    };
    HttpGraphqlResponse::from_stream(
        streaming_format,
        // Not perfect for the errors count, but good enough to detect a request error
        first_response.status(),
        futures_util::stream::iter(std::iter::once(first_response)).chain(stream),
    )
}

impl<'ctx, R: Runtime> PreExecutionContext<'ctx, R> {
    async fn execute_single(mut self, request: Request) -> (Option<OperationMetricsAttributes>, Response) {
        let operation_plan = match self.prepare_operation(request).await {
            Ok(operation_plan) => operation_plan,
            Err((metadata, response)) => return (metadata, response),
        };

        let metrics_attributes = Some(operation_plan.metrics_attributes.clone());
        let response = if matches!(operation_plan.ty(), OperationType::Subscription) {
            Response::pre_execution_error(GraphqlError::new(
                "Subscriptions are only suported on streaming transports. Try making a request with SSE or WebSockets",
                ErrorCode::BadRequest,
            ))
        } else {
            self.execute_query_or_mutation(operation_plan).await
        };

        (metrics_attributes, response)
    }

    async fn execute_stream(
        mut self,
        request: Request,
        mut sender: mpsc::Sender<Response>,
    ) -> (Option<OperationMetricsAttributes>, GraphqlResponseStatus) {
        let operation_plan = match self.prepare_operation(request).await {
            Ok(operation_plan) => operation_plan,
            Err((metadata, response)) => {
                let status = response.status();
                sender.send(response).await.ok();
                return (metadata, status);
            }
        };
        let operation_type = operation_plan.ty();
        let metrics_attributes = Some(operation_plan.metrics_attributes.clone());

        if matches!(operation_type, OperationType::Query | OperationType::Mutation) {
            let response = self.execute_query_or_mutation(operation_plan).await;
            let status = response.status();
            sender.send(response).await.ok();
            return (metrics_attributes, status);
        }

        let mut status: GraphqlResponseStatus = GraphqlResponseStatus::Success;
        struct Sender<'a> {
            sender: mpsc::Sender<Response>,
            status: &'a mut GraphqlResponseStatus,
        }

        impl crate::execution::ResponseSender for Sender<'_> {
            type Error = mpsc::SendError;
            async fn send(&mut self, response: Response) -> Result<(), Self::Error> {
                *self.status = self.status.union(response.status());
                self.sender.send(response).await
            }
        }

        self.execute_subscription(
            operation_plan,
            Sender {
                sender,
                status: &mut status,
            },
        )
        .await;
        (metrics_attributes, status)
    }

    async fn prepare_operation(
        &mut self,
        mut request: Request,
    ) -> Result<ExecutableOperation, (Option<OperationMetricsAttributes>, Response)> {
        let result = {
            let PreparedOperationDocument {
                cache_key,
                document_fut,
            } = match self.prepare_operation_document(&request) {
                Ok(pq) => pq,
                Err(err) => return Err((None, Response::pre_execution_error(err))),
            };

            if let Some(operation) = self.operation_cache.get(&cache_key).await {
                Ok(operation)
            } else if let Some(document_fut) = document_fut {
                match document_fut.await {
                    Ok(document) => Err((cache_key, Some(document))),
                    Err(err) => return Err((None, Response::pre_execution_error(err))),
                }
            } else {
                Err((cache_key, None))
            }
        };

        let operation = match result {
            Ok(operation) => operation,
            Err((cache_key, document)) => {
                if let Some(query) = document {
                    request.query = query
                }
                let operation = Operation::build(&self.schema, &request)
                    .map(Arc::new)
                    .map_err(|mut err| (err.take_metrics_attributes(), Response::pre_execution_error(err)))?;

                self.push_background_future(self.engine.operation_cache.insert(cache_key, operation.clone()).boxed());
                operation
            }
        };

        let variables = Variables::build(self.schema.as_ref(), &operation, request.variables).map_err(|errors| {
            (
                Some(operation.metrics_attributes.clone()),
                Response::pre_execution_errors(errors),
            )
        })?;

        self.finalize_operation(Arc::clone(&operation), variables)
            .await
            .map_err(|err| {
                (
                    Some(operation.metrics_attributes.clone()),
                    Response::pre_execution_error(err),
                )
            })
    }
}

pub struct Session<R: Runtime> {
    engine: Arc<Engine<R>>,
    request_context: Arc<RequestContext<<R::Hooks as Hooks>::Context>>,
}

impl<R: Runtime> Clone for Session<R> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            request_context: Arc::clone(&self.request_context),
        }
    }
}

pub(crate) struct RequestContext<C> {
    pub headers: http::HeaderMap,
    pub streaming_format: Option<StreamingFormat>,
    pub client: Option<Client>,
    pub access_token: AccessToken,
    pub hooks_context: C,
}

impl<R: Runtime> Session<R> {
    pub fn execute_websocket(&self, id: String, request: Request) -> impl Stream<Item = websocket::Message> {
        self.engine
            .execute_stream(self.request_context.clone(), request)
            .map(move |response| match response {
                Response::PreExecutionError(_) => websocket::Message::Error {
                    id: id.clone(),
                    payload: websocket::Payload(response),
                },
                response => websocket::Message::Next {
                    id: id.clone(),
                    payload: websocket::Payload(response),
                },
            })
    }
}
