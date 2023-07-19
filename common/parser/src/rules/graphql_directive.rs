use dynaql::registry::{ConnectorHeaderValue, ConnectorHeaders};
use dynaql_parser::types::SchemaDefinition;
use tracing::warn;
use url::Url;

use crate::directive_de::parse_directive;

use super::{
    connector_headers::{Header, IntrospectionHeader},
    connector_transforms::Transforms,
    directive::Directive,
    visitor::{Visitor, VisitorContext},
};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlDirective {
    /// A unique identifier for the given directive.
    ///
    /// This ID *MUST NOT* be persisted (and defaults to `None` when deserializing), as the ID is
    /// re-generated whenever the schema is parsed.
    #[serde(skip)]
    pub id: Option<u16>,

    /// The namespace within which the upstream GraphQL schema is embedded.
    ///
    /// If unset, a namespace is auto-generated based on the `id`, or an error is returned if no
    /// `id` is defined.
    namespace: Option<String>,

    /// The name of the connector.
    ///
    /// See the `namespace` field for more details.
    ///
    /// # Deprecation
    ///
    /// This field was renamed to `namespace`, to better align with its intent.
    ///
    /// If this field exists in the schema, a warning is logged, until a future date at which point
    /// an error is returned.
    ///
    /// If both fields exist, `namespace` is used over `name`, a warning is logged, `namespace` is
    /// used over `name`, until a future date, at which point an error is returned.
    #[serde(default)]
    #[serde(deserialize_with = "deprecated_name")]
    name: Option<String>,

    pub url: Url,
    #[serde(default)]
    headers: Vec<Header>,
    #[serde(default)]
    introspection_headers: Option<Vec<IntrospectionHeader>>,

    #[serde(default)]
    pub transforms: Option<Transforms>,
}

fn deprecated_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let name: Option<String> = serde::de::Deserialize::deserialize(deserializer)?;

    if name.is_some() {
        warn!("`name` field on `@graphql` directive is deprecated. Use `namespace` instead.");
    }

    Ok(name)
}

impl GraphqlDirective {
    pub fn headers(&self) -> ConnectorHeaders {
        ConnectorHeaders::new(
            self.headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone())),
        )
    }

    pub fn introspection_headers(&self) -> Vec<(&str, &str)> {
        match &self.introspection_headers {
            Some(introspection_headers) => introspection_headers
                .iter()
                .map(|header| (header.name.as_str(), header.value.as_str()))
                .collect(),

            None => self
                .headers
                .iter()
                .filter_map(|header| match &header.value {
                    ConnectorHeaderValue::Static(value) => Some((header.name.as_str(), value.as_str())),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The optional *namespace* for the given GraphQL directive.
    ///
    /// This will default to the `namespace` field if present, or the (deprecated) `name` field
    /// otherwise.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref().or(self.name.as_deref())
    }
}

const GRAPHQL_DIRECTIVE_NAME: &str = "graphql";

impl Directive for GraphqlDirective {
    fn definition() -> String {
        r#"
        directive @graphql(
          """
          The name of the upstream GraphQL source.
          """
          name: String!

          """
          The URL of the GraphQL source.
          """
          url: Url!

          """
          Optional headers to embed in every HTTP request.
          """
          headers: [GraphqlHeader!]

          """
          Optional headers to embed in an introspection HTTP request.
          """
          introspectionHeaders: [GraphqlIntrospectionHeader!]
        ) on SCHEMA

        input GraphqlIntrospectionHeader {
            name: String!
            value: String!
        }

        input GraphqlHeader {
            name: String!
            value: String
            forward: String
        }
        "#
        .to_string()
    }
}

pub struct GraphqlVisitor;

impl<'a> Visitor<'a> for GraphqlVisitor {
    fn enter_schema(&mut self, ctx: &mut VisitorContext<'a>, doc: &'a dynaql::Positioned<SchemaDefinition>) {
        let directives = doc
            .node
            .directives
            .iter()
            .filter(|d| d.node.name.node == GRAPHQL_DIRECTIVE_NAME);

        for directive in directives {
            match parse_directive::<GraphqlDirective>(&directive.node, ctx.variables) {
                Ok(parsed_directive) => {
                    ctx.graphql_directives.push((parsed_directive, directive.pos));
                }
                Err(err) => ctx.report_error(vec![directive.pos], err.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::executor::block_on;

    use crate::{connector_parsers::MockConnectorParsers, rules::visitor::RuleError};

    #[test]
    fn parsing_graphql_directive() {
        let variables = HashMap::from([
            ("MY_API_KEY".to_owned(), "i_am_a_key".to_owned()),
            ("ADMIN_USER_ID".to_owned(), "root".to_owned()),
        ]);

        let connector_parsers = MockConnectorParsers::default();

        let schema = r#"
            extend schema
              @graphql(
                name: "countries",
                url: "https://countries.trevorblades.com",
                headers: [{ name: "authorization", value: "Bearer {{ env.MY_API_KEY }}"}],
                introspectionHeaders: [{ name: "x-user-id", value: "{{ env.ADMIN_USER_ID }}"}]
              )
            "#;

        block_on(crate::parse(schema, &variables, &connector_parsers)).unwrap();

        insta::assert_debug_snapshot!(connector_parsers.graphql_directives.lock().unwrap(), @r###"
        [
            GraphqlDirective {
                id: Some(
                    0,
                ),
                namespace: None,
                name: Some(
                    "countries",
                ),
                url: Url {
                    scheme: "https",
                    cannot_be_a_base: false,
                    username: "",
                    password: None,
                    host: Some(
                        Domain(
                            "countries.trevorblades.com",
                        ),
                    ),
                    port: None,
                    path: "/",
                    query: None,
                    fragment: None,
                },
                headers: [
                    Header {
                        name: "authorization",
                        value: Static(
                            "Bearer i_am_a_key",
                        ),
                    },
                ],
                introspection_headers: Some(
                    [
                        IntrospectionHeader {
                            name: "x-user-id",
                            value: "root",
                        },
                    ],
                ),
                transforms: None,
            },
        ]
        "###);
    }

    #[test]
    fn parsing_unnamed_graphql_directive() {
        let variables = HashMap::from([
            ("MY_API_KEY".to_owned(), "i_am_a_key".to_owned()),
            ("ADMIN_USER_ID".to_owned(), "root".to_owned()),
        ]);

        let connector_parsers = MockConnectorParsers::default();

        let schema = r#"
            extend schema
              @graphql(
                url: "https://countries.trevorblades.com",
                headers: [{ name: "authorization", value: "Bearer {{ env.MY_API_KEY }}"}],
                introspectionHeaders: [{ name: "x-user-id", value: "{{ env.ADMIN_USER_ID }}"}]
              )
            "#;

        block_on(crate::parse(schema, &variables, &connector_parsers)).unwrap();

        insta::assert_debug_snapshot!(connector_parsers.graphql_directives.lock().unwrap(), @r###"
        [
            GraphqlDirective {
                id: Some(
                    0,
                ),
                namespace: None,
                name: None,
                url: Url {
                    scheme: "https",
                    cannot_be_a_base: false,
                    username: "",
                    password: None,
                    host: Some(
                        Domain(
                            "countries.trevorblades.com",
                        ),
                    ),
                    port: None,
                    path: "/",
                    query: None,
                    fragment: None,
                },
                headers: [
                    Header {
                        name: "authorization",
                        value: Static(
                            "Bearer i_am_a_key",
                        ),
                    },
                ],
                introspection_headers: Some(
                    [
                        IntrospectionHeader {
                            name: "x-user-id",
                            value: "root",
                        },
                    ],
                ),
                transforms: None,
            },
        ]
        "###);
    }

    macro_rules! assert_validation_error {
        ($schema:literal, $expected_message:literal) => {
            assert_matches!(
                crate::parse_registry($schema)
                    .err()
                    .and_then(crate::Error::validation_errors)
                    // We don't care whether there are more errors or not.
                    // It only matters that we find the expected error.
                    .and_then(|errors| errors.into_iter().next()),
                Some(RuleError { message, .. }) => {
                    assert_eq!(message, $expected_message);
                }
            );
        };
    }

    #[test]
    fn missing_field() {
        assert_validation_error!(
            r#"
            extend schema
              @graphql(
                name: "countries",
                headers: [{ name: "authorization", value: "..."}],
              )
            "#,
            "missing field `url`"
        );
    }

    #[test]
    fn invalid_header_name_type() {
        assert_validation_error!(
            r#"
            extend schema
              @graphql(
                name: "countries",
                url: "https://countries.trevorblades.com",
                headers: [{ name: 12, value: "..."}],
              )
            "#,
            "[6:26] invalid type: integer `12`, expected a string"
        );
    }

    #[test]
    fn invalid_introspection_header_name_type() {
        assert_validation_error!(
            r#"
            extend schema
              @graphql(
                name: "countries",
                url: "https://countries.trevorblades.com",
                introspectionHeaders: [{ name: 12, value: "..."}],
              )
            "#,
            "[6:39] invalid type: integer `12`, expected a string"
        );
    }

    #[test]
    fn test_no_introspection_headers() {
        let schema = r#"
            extend schema
              @graphql(
                name: "countries",
                url: "https://countries.trevorblades.com",
                headers: [{ name: "authorization", value: "Bearer blah"}],
                introspectionHeaders: []
              )
            "#;

        let connector_parsers = MockConnectorParsers::default();

        block_on(crate::parse(schema, &HashMap::new(), &connector_parsers)).unwrap();

        assert_eq!(
            connector_parsers.graphql_directives.lock().unwrap()[0].introspection_headers(),
            vec![]
        );
    }

    #[test]
    fn test_introspection_headers_inherits_headers_by_default() {
        let schema = r#"
            extend schema
              @graphql(
                name: "countries",
                url: "https://countries.trevorblades.com",
                headers: [{ name: "authorization", value: "Bearer blah"}],
              )
            "#;

        let connector_parsers = MockConnectorParsers::default();

        block_on(crate::parse(schema, &HashMap::new(), &connector_parsers)).unwrap();

        assert_eq!(
            connector_parsers.graphql_directives.lock().unwrap()[0].introspection_headers(),
            vec![("authorization", "Bearer blah")]
        );
    }
}
