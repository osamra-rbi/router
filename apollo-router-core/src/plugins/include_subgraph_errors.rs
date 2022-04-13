use crate::error::Error as SubgraphError;
use crate::plugin::Plugin;
use crate::{register_plugin, SubgraphRequest, SubgraphResponse};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use tower::util::BoxService;
use tower::{BoxError, ServiceExt};

#[allow(clippy::field_reassign_with_default)]
static REDACTED_ERROR_MESSAGE: Lazy<Vec<SubgraphError>> = Lazy::new(|| {
    let mut error: SubgraphError = Default::default();

    error.message = "Subgraph errors redacted".to_string();

    vec![error]
});

register_plugin!(
    "experimental",
    "include_subgraph_errors",
    IncludeSubgraphErrors
);

#[derive(Clone, Debug, JsonSchema, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Config {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    subgraphs: HashMap<String, bool>,
}

struct IncludeSubgraphErrors {
    config: Config,
}

impl Plugin for IncludeSubgraphErrors {
    type Config = Config;

    fn new(config: Self::Config) -> Result<Self, BoxError> {
        Ok(IncludeSubgraphErrors { config })
    }

    fn subgraph_service(
        &mut self,
        name: &str,
        service: BoxService<SubgraphRequest, SubgraphResponse, BoxError>,
    ) -> BoxService<SubgraphRequest, SubgraphResponse, BoxError> {
        // Search for subgraph in our configured subgraph map.
        // If we can't find it, use the "all" value
        if !*self.config.subgraphs.get(name).unwrap_or(&self.config.all) {
            return service
                .map_response(move |mut response: SubgraphResponse| {
                    if !response.response.body().errors.is_empty() {
                        response.response.body_mut().errors = REDACTED_ERROR_MESSAGE.clone();
                    }
                    response
                })
                .boxed();
        }
        service
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::plugin_utils::mock::subgraph::MockSubgraph;
    use crate::{
        plugin_utils, DynPlugin, Object, PluggableRouterServiceBuilder, ResponseBody,
        RouterRequest, RouterResponse, Schema,
    };
    use serde_json::Value as jValue;
    use serde_json_bytes::{ByteString, Value};
    use std::sync::Arc;
    use tower::{util::BoxCloneService, Service};

    static UNREDACTED_PRODUCT_RESPONSE: Lazy<ResponseBody> = Lazy::new(|| {
        ResponseBody::GraphQL(serde_json::from_str(r#"{"data": {"topProducts":null}, "errors":[{"message": "couldn't find mock for query", "locations": [], "path": null, "extensions": { "test": "value" }}]}"#).unwrap())
    });

    static REDACTED_PRODUCT_RESPONSE: Lazy<ResponseBody> = Lazy::new(|| {
        ResponseBody::GraphQL(serde_json::from_str(r#"{"data": {"topProducts":null}, "errors":[{"message": "Subgraph errors redacted", "locations": [], "path": null, "extensions": {}}]}"#).unwrap())
    });

    static REDACTED_ACCOUNT_RESPONSE: Lazy<ResponseBody> = Lazy::new(|| {
        ResponseBody::GraphQL(serde_json::from_str(r#"{"data": null , "errors":[{"message": "Subgraph errors redacted", "locations": [], "path": null, "extensions": {}}]}"#).unwrap())
    });

    static EXPECTED_RESPONSE: Lazy<ResponseBody> = Lazy::new(|| {
        ResponseBody::GraphQL(serde_json::from_str(r#"{"data":{"topProducts":[{"upc":"1","name":"Table","reviews":[{"id":"1","product":{"name":"Table"},"author":{"id":"1","name":"Ada Lovelace"}},{"id":"4","product":{"name":"Table"},"author":{"id":"2","name":"Alan Turing"}}]},{"upc":"2","name":"Couch","reviews":[{"id":"2","product":{"name":"Couch"},"author":{"id":"1","name":"Ada Lovelace"}}]}]}}"#).unwrap())
    });

    static VALID_QUERY: &str = r#"query TopProducts($first: Int) { topProducts(first: $first) { upc name reviews { id product { name } author { id name } } } }"#;

    static ERROR_PRODUCT_QUERY: &str = r#"query ErrorTopProducts($first: Int) { topProducts(first: $first) { upc name reviews { id product { name } author { id name } } } }"#;

    static ERROR_ACCOUNT_QUERY: &str = r#"query Query { me { name }}"#;

    async fn execute_router_test(
        query: &str,
        body: &ResponseBody,
        mut router_service: BoxCloneService<RouterRequest, RouterResponse, BoxError>,
    ) {
        let request = plugin_utils::RouterRequest::builder()
            .query(query.to_string())
            .variables(Arc::new(
                vec![(ByteString::from("first"), Value::Number(2usize.into()))]
                    .into_iter()
                    .collect(),
            ))
            .build()
            .into();

        let response = router_service
            .ready()
            .await
            .unwrap()
            .call(request)
            .await
            .unwrap();
        assert_eq!(response.response.body(), body);
    }

    async fn build_mock_router(
        plugin: Box<dyn DynPlugin>,
    ) -> BoxCloneService<RouterRequest, RouterResponse, BoxError> {
        let mut extensions = Object::new();
        extensions.insert("test", Value::String(ByteString::from("value")));

        let account_mocks = vec![
            (
                r#"{"query":"query TopProducts__accounts__3($representations:[_Any!]!){_entities(representations:$representations){...on User{name}}}","operationName":"TopProducts__accounts__3","variables":{"representations":[{"__typename":"User","id":"1"},{"__typename":"User","id":"2"},{"__typename":"User","id":"1"}]}}"#,
                r#"{"data":{"_entities":[{"name":"Ada Lovelace"},{"name":"Alan Turing"},{"name":"Ada Lovelace"}]}}"#
            )
        ].into_iter().map(|(query, response)| (serde_json::from_str(query).unwrap(), serde_json::from_str(response).unwrap())).collect();
        let account_service = MockSubgraph::new(account_mocks);

        let review_mocks = vec![
            (
                r#"{"query":"query TopProducts__reviews__1($representations:[_Any!]!){_entities(representations:$representations){...on Product{reviews{id product{__typename upc}author{__typename id}}}}}","operationName":"TopProducts__reviews__1","variables":{"representations":[{"__typename":"Product","upc":"1"},{"__typename":"Product","upc":"2"}]}}"#,
                r#"{"data":{"_entities":[{"reviews":[{"id":"1","product":{"__typename":"Product","upc":"1"},"author":{"__typename":"User","id":"1"}},{"id":"4","product":{"__typename":"Product","upc":"1"},"author":{"__typename":"User","id":"2"}}]},{"reviews":[{"id":"2","product":{"__typename":"Product","upc":"2"},"author":{"__typename":"User","id":"1"}}]}]}}"#
            )
            ].into_iter().map(|(query, response)| (serde_json::from_str(query).unwrap(), serde_json::from_str(response).unwrap())).collect();
        let review_service = MockSubgraph::new(review_mocks);

        let product_mocks = vec![
            (
                r#"{"query":"query TopProducts__products__0($first:Int){topProducts(first:$first){__typename upc name}}","operationName":"TopProducts__products__0","variables":{"first":2}}"#,
                r#"{"data":{"topProducts":[{"__typename":"Product","upc":"1","name":"Table"},{"__typename":"Product","upc":"2","name":"Couch"}]}}"#
            ),
            (
                r#"{"query":"query TopProducts__products__2($representations:[_Any!]!){_entities(representations:$representations){...on Product{name}}}","operationName":"TopProducts__products__2","variables":{"representations":[{"__typename":"Product","upc":"1"},{"__typename":"Product","upc":"1"},{"__typename":"Product","upc":"2"}]}}"#,
                r#"{"data":{"_entities":[{"name":"Table"},{"name":"Table"},{"name":"Couch"}]}}"#
            )
            ].into_iter().map(|(query, response)| (serde_json::from_str(query).unwrap(), serde_json::from_str(response).unwrap())).collect();

        let product_service = MockSubgraph::new(product_mocks).with_extensions(extensions);

        let schema: Arc<Schema> = Arc::new(
            include_str!("../../../apollo-router-benchmarks/benches/fixtures/supergraph.graphql")
                .parse()
                .unwrap(),
        );

        let builder = PluggableRouterServiceBuilder::new(schema.clone());

        let builder = builder
            .with_dyn_plugin("experimental.include_subgraph_errors".to_string(), plugin)
            .with_subgraph_service("accounts", account_service.clone())
            .with_subgraph_service("reviews", review_service.clone())
            .with_subgraph_service("products", product_service.clone());

        let (router, _) = builder.build().await.expect("should build");

        router
    }

    fn get_redacting_plugin(config: &jValue) -> Box<dyn DynPlugin> {
        // Build a redacting plugin
        crate::plugins()
            .get("experimental.include_subgraph_errors")
            .expect("Plugin not found")
            .create_instance(config)
            .expect("Plugin not created")
    }

    #[tokio::test]
    async fn it_returns_valid_response() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({ "all": false }));
        let router = build_mock_router(plugin).await;
        execute_router_test(VALID_QUERY, &*EXPECTED_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_redacts_all_subgraphs_explicit_redact() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({ "all": false }));
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*REDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_redacts_all_subgraphs_implicit_redact() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({}));
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*REDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_not_redact_all_subgraphs_explicit_allow() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({ "all": true }));
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*UNREDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_not_redact_all_implicit_redact_product_explict_allow_for_product_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({ "subgraphs": {"products": true }}));
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*UNREDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_redact_all_implicit_redact_product_explict_allow_for_review_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(&serde_json::json!({ "subgraphs": {"reviews": true }}));
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*REDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_not_redact_all_explicit_allow_review_explict_redact_for_product_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(
            &serde_json::json!({ "all": true, "subgraphs": {"reviews": false }}),
        );
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*UNREDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_redact_all_explicit_allow_product_explict_redact_for_product_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(
            &serde_json::json!({ "all": true, "subgraphs": {"products": false }}),
        );
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*REDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_not_redact_all_explicit_allow_account_explict_redact_for_product_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(
            &serde_json::json!({ "all": true, "subgraphs": {"accounts": false }}),
        );
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_PRODUCT_QUERY, &*UNREDACTED_PRODUCT_RESPONSE, router).await;
    }

    #[tokio::test]
    async fn it_does_redact_all_explicit_allow_account_explict_redact_for_account_query() {
        // Build a redacting plugin
        let plugin = get_redacting_plugin(
            &serde_json::json!({ "all": true, "subgraphs": {"accounts": false }}),
        );
        let router = build_mock_router(plugin).await;
        execute_router_test(ERROR_ACCOUNT_QUERY, &*REDACTED_ACCOUNT_RESPONSE, router).await;
    }
}