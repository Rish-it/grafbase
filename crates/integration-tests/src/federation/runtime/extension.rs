use std::{collections::HashMap, sync::Arc};

use engine_schema::{Subgraph, SubgraphId};
use extension_catalog::{Extension, ExtensionCatalog, ExtensionId, Id, Manifest};
use runtime::{
    error::{ErrorResponse, PartialGraphqlError},
    extension::{Data, ExtensionDirective},
    hooks::{Anything, DynHookContext, EdgeDefinition},
};
use tokio::sync::Mutex;
use url::Url;

pub struct TestExtensions {
    pub tmpdir: tempfile::TempDir,
    catalog: ExtensionCatalog,
    builders: HashMap<ExtensionId, Box<dyn TestExtensionBuilder>>,
    global_instances: Mutex<HashMap<ExtensionId, Arc<dyn TestExtension>>>,
    subgraph_instances: Mutex<HashMap<(ExtensionId, SubgraphId), Arc<dyn TestExtension>>>,
}

impl Default for TestExtensions {
    fn default() -> Self {
        Self {
            tmpdir: tempfile::tempdir().unwrap(),
            catalog: Default::default(),
            builders: Default::default(),
            global_instances: Default::default(),
            subgraph_instances: Default::default(),
        }
    }
}

impl TestExtensions {
    #[track_caller]
    pub fn push_extension<E: TestExtensionBuilder + Sized + Default>(&mut self) {
        let config = E::config();

        let manifest = extension_catalog::Manifest {
            id: E::id(),
            kind: config.kind,
            sdk_version: "0.0.0".parse().unwrap(),
            minimum_gateway_version: "0.0.0".parse().unwrap(),
            sdl: config.sdl.map(str::to_string),
        };
        let dir = self.tmpdir.path().join(manifest.id.to_string());
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest.clone().into_versioned()).unwrap(),
        )
        .unwrap();
        let id = self.catalog.push(Extension {
            manifest,
            wasm_path: Default::default(),
        });
        self.builders.insert(id, Box::new(E::default()));
    }

    pub fn catalog(&self) -> &ExtensionCatalog {
        &self.catalog
    }

    pub fn iter(&self) -> impl Iterator<Item = (Url, &Manifest)> {
        self.catalog.iter().map(move |ext| {
            (
                Url::from_file_path(self.tmpdir.path().join(ext.manifest.id.to_string())).unwrap(),
                &ext.manifest,
            )
        })
    }
}

pub struct TestExtensionConfig {
    pub sdl: Option<&'static str>,
    pub kind: extension_catalog::Kind,
}

#[allow(unused_variables)] // makes it easier to copy-paste relevant functions
pub trait TestExtensionBuilder: Send + Sync + 'static {
    fn id() -> Id
    where
        Self: Sized;

    fn config() -> TestExtensionConfig
    where
        Self: Sized;

    fn build(&self, schema_directives: Vec<ExtensionDirective<'_, serde_json::Value>>) -> Arc<dyn TestExtension>;
}

#[allow(unused_variables)] // makes it easier to copy-paste relevant functions
#[async_trait::async_trait]
pub trait TestExtension: Send + Sync + 'static {
    async fn resolve<'a>(
        &self,
        context: &DynHookContext,
        field: EdgeDefinition<'a>,
        directive: ExtensionDirective<'a, serde_json::Value>,
        inputs: Vec<serde_json::Value>,
    ) -> Result<Vec<Result<serde_json::Value, PartialGraphqlError>>, PartialGraphqlError> {
        Err(PartialGraphqlError::internal_extension_error())
    }
}

impl runtime::extension::ExtensionRuntime for TestExtensions {
    type SharedContext = DynHookContext;

    async fn resolve_field<'a>(
        &self,
        extension_id: ExtensionId,
        subgraph: Subgraph<'a>,
        context: &Self::SharedContext,
        field: EdgeDefinition<'a>,
        directive: ExtensionDirective<'a, impl Anything<'a>>,
        inputs: impl IntoIterator<Item: Anything<'a>> + Send,
    ) -> Result<Vec<Result<runtime::extension::Data, PartialGraphqlError>>, PartialGraphqlError> {
        let instance = self
            .subgraph_instances
            .lock()
            .await
            .entry((extension_id, subgraph.id()))
            .or_insert_with(|| {
                self.builders.get(&extension_id).unwrap().build(
                    subgraph
                        .extension_schema_directives()
                        .filter(|dir| dir.extension_id == extension_id)
                        .map(|dir| ExtensionDirective {
                            name: dir.name(),
                            static_arguments: serde_json::to_value(dir.static_arguments()).unwrap(),
                        })
                        .collect(),
                )
            })
            .clone();

        instance
            .resolve(
                context,
                field,
                ExtensionDirective {
                    name: directive.name,
                    static_arguments: serde_json::to_value(directive.static_arguments).unwrap(),
                },
                inputs
                    .into_iter()
                    .map(serde_json::to_value)
                    .collect::<Result<_, _>>()
                    .unwrap(),
            )
            .await
            .map(|items| {
                items
                    .into_iter()
                    .map(|res| res.map(|value| Data::JsonBytes(serde_json::to_vec(&value).unwrap())))
                    .collect()
            })
    }

    async fn authenticate(
        &self,
        extension_id: ExtensionId,
        _authorizer_id: runtime::extension::AuthorizerId,
        _headers: http::HeaderMap,
    ) -> Result<(http::HeaderMap, HashMap<String, serde_json::Value>), ErrorResponse> {
        let _instance = self
            .global_instances
            .lock()
            .await
            .entry(extension_id)
            .or_insert_with(|| self.builders.get(&extension_id).unwrap().build(Vec::new()))
            .clone();
        Err(ErrorResponse {
            status: http::StatusCode::INTERNAL_SERVER_ERROR,
            errors: Vec::new(),
        })
    }
}
