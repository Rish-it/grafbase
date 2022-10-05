use super::visitor::{Visitor, VisitorContext};
use dynaql::Positioned;
use dynaql_parser::types::{FieldDefinition, TypeDefinition};

pub const DEFAULT_DIRECTIVE: &str = "default";
pub const VALUE_ARGUMENT: &str = "value";

pub struct DefaultDirective;

// FIXME: Validate that the type of the default constant value is compatible with the type of the field.

const FIELDS_NOT_ALLOWED: &[&str] = &["id"];

impl<'a> Visitor<'a> for DefaultDirective {
    fn directives(&self) -> String {
        r#"
        directive @default(value: String) on FIELD_DEFINITION
        "#
        .to_string()
    }

    fn enter_field(
        &mut self,
        ctx: &mut VisitorContext<'a>,
        field: &'a Positioned<FieldDefinition>,
        _parent_type: &'a Positioned<TypeDefinition>,
    ) {
        if let Some(directive) = field
            .node
            .directives
            .iter()
            .find(|d| d.node.name.node == DEFAULT_DIRECTIVE)
        {
            if crate::utils::is_modelized_node(&ctx.types, &field.node.ty.node).is_some() {
                ctx.report_error(
                    vec![directive.pos],
                    format!("The @default directive is not accepted on fields referring to other models"),
                );
            }

            if let Some(field) = FIELDS_NOT_ALLOWED
                .iter()
                .copied()
                .find(|field_name| field.node.name.node == *field_name)
            {
                ctx.report_error(
                    vec![directive.pos],
                    format!("The @default directive is not accepted on the `{field}` field"),
                );
            }

            let arguments: Vec<_> = directive
                .node
                .arguments
                .iter()
                .map(|(key, value)| (key.node.as_str(), value))
                .collect();
            if let [("value", value)] = arguments.as_slice() {
                let error = {
                    let ctx_registry = ctx.registry.borrow();
                    dynaql::validation::utils::is_valid_input_value(
                        &ctx_registry,
                        &field.node.ty.node.to_string(),
                        &value.node,
                        dynaql::QueryPathNode {
                            parent: None,
                            segment: dynaql::QueryPathSegment::Name(&field.node.name.node),
                        },
                    )
                };
                if let Some(err) = error {
                    ctx.report_error(
                        vec![directive.pos],
                        format!("The @default value is of a wrong type: {err}"),
                    );
                }
            } else {
                ctx.report_error(
                    vec![directive.pos],
                    "The @default directive takes a single `value` argument".to_string(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::visitor::visit;
    use dynaql_parser::parse_schema;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_default_wrong_argument_name() {
        let schema = r#"
            type Product @model {
                id: ID!
                name: String @default(foo: "default")
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 1);
        assert_eq!(
            ctx.errors.get(0).unwrap().message,
            "The @default directive takes a single `value` argument",
        );
    }

    #[test]
    fn test_default_missing_argument() {
        let schema = r#"
            type Product @model {
                id: ID!
                name: String @default
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 1);
        assert_eq!(
            ctx.errors.get(0).unwrap().message,
            "The @default directive takes a single `value` argument",
        );
    }

    #[test]
    fn test_default_on_id_field() {
        let schema = r#"
            type Product @model {
                id: ID! @default(value: "default")
                name: String
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 1);
        assert_eq!(
            ctx.errors.get(0).unwrap().message,
            "The @default directive is not accepted on the `id` field",
        );
    }

    #[test]
    fn test_default_model_field() {
        let schema = r#"
            type Category @model {
                id: ID!
                name: String!
            }

            type Product @model {
                id: ID!
                name: String!
                category: Category @default(value: null)
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 1);
        assert_eq!(
            ctx.errors.get(0).unwrap().message,
            "The @default directive is not accepted on fields referring to other models",
        );
    }

    #[test]
    fn test_wrong_value_type() {
        let schema = r#"
            type Product @model {
                id: ID!
                name: String! @default(value: 10)
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 1);
        assert_eq!(
            ctx.errors.get(0).unwrap().message,
            "The @default value is of a wrong type: \"name\", expected type \"String\"",
        );
    }

    #[test]
    fn test_valid() {
        let schema = r#"
            type Product @model {
                id: ID!
                name: String @default(value: "foo")
            }
            "#;

        let schema = parse_schema(schema).unwrap();
        let mut ctx = VisitorContext::new(&schema);
        visit(&mut DefaultDirective, &mut ctx, &schema);

        assert_eq!(ctx.errors.len(), 0);
    }
}
