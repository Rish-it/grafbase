use crate::rules::visitor::VisitorContext;
use crate::utils::to_input_type;
use async_graphql::indexmap::IndexMap;
use async_graphql::registry::transformers::Transformer;
use async_graphql::registry::{
    resolvers::context_data::ContextDataResolver, resolvers::dynamo_mutation::DynamoMutationResolver,
    resolvers::dynamo_querying::DynamoResolver, resolvers::Resolver, resolvers::ResolverType,
    variables::VariableResolveDefinition, MetaField, MetaInputValue, MetaType,
};
use async_graphql_parser::types::{FieldDefinition, ObjectType};
use case::CaseExt;

/// Create an input type for a non_primitive Type.
pub fn add_input_type_non_primitive<'a>(ctx: &mut VisitorContext<'a>, object: &ObjectType, type_name: &str) -> String {
    let type_name = type_name.to_string();
    let input_type = format!("{}Input", type_name.to_camel());

    // Input
    ctx.registry.get_mut().create_type(
        &mut |_| MetaType::InputObject {
            name: input_type.clone(),
            description: Some(format!("{} input type.", type_name)),
            oneof: false,
            input_fields: {
                let mut input_fields = IndexMap::new();
                for field in &object.fields {
                    let name = &field.node.name.node;

                    input_fields.insert(
                        name.clone().to_string(),
                        MetaInputValue {
                            name: name.to_string(),
                            description: field.node.description.clone().map(|x| x.node),
                            ty: to_input_type(&ctx.types, field.node.ty.clone().node).to_string(),
                            visible: None,
                            default_value: None,
                            is_secret: false,
                        },
                    );
                }
                input_fields
            },
            visible: None,
            rust_typename: type_name.clone(),
        },
        &input_type,
        &input_type,
    );

    input_type
}

/// Add the create Mutation for a given Object
pub fn add_create_mutation<'a>(
    ctx: &mut VisitorContext<'a>,
    object: &ObjectType,
    id_field: &FieldDefinition,
    type_name: &str,
) {
    let type_name = type_name.to_string();
    let create_input_name = format!("{}CreateInput", type_name.to_camel());
    let create_payload_name = format!("{}CreatePayload", type_name.to_camel());
    // CreateInput
    ctx.registry.get_mut().create_type(
        &mut |_| MetaType::InputObject {
            name: create_input_name.clone(),
            description: Some(format!("Input to create a new {}", type_name)),
            oneof: false,
            input_fields: {
                let mut input_fields = IndexMap::new();
                // As we are sure there are primitives types
                for field in &object.fields {
                    let name = &field.node.name.node;
                    // We prevent the id field from appearing inside a create mutation.
                    // Right now: id must be autogenerated by Grafbase.
                    if name.ne(&id_field.name.node) {
                        input_fields.insert(
                            name.clone().to_string(),
                            MetaInputValue {
                                name: name.to_string(),
                                description: field.node.description.clone().map(|x| x.node),
                                ty: to_input_type(&ctx.types, field.node.ty.clone().node).to_string(),
                                visible: None,
                                default_value: None,
                                is_secret: false,
                            },
                        );
                    }
                }
                input_fields
            },
            visible: None,
            rust_typename: type_name.clone(),
        },
        &create_input_name,
        &create_input_name,
    );

    // CreatePayload
    ctx.registry.get_mut().create_type(
        &mut |_| MetaType::Object {
            name: create_payload_name.clone(),
            description: None,
            fields: {
                let mut fields = IndexMap::new();
                let name = type_name.to_lowercase();
                fields.insert(
                    name.clone(),
                    MetaField {
                        name,
                        description: None,
                        args: Default::default(),
                        ty: type_name.to_camel(),
                        deprecation: Default::default(),
                        cache_control: Default::default(),
                        external: false,
                        requires: None,
                        provides: None,
                        visible: None,
                        compute_complexity: None,
                        is_edge: None,
                        resolve: Some(Resolver {
                            id: Some(format!("{}_resolver", type_name.to_lowercase())),
                            // Single entity
                            r#type: ResolverType::DynamoResolver(DynamoResolver::QueryPKSK {
                                pk: VariableResolveDefinition::LocalData("id".to_string()),
                                sk: VariableResolveDefinition::LocalData("id".to_string()),
                            }),
                        }),
                        transforms: None,
                    },
                );
                fields
            },
            cache_control: async_graphql::CacheControl {
                public: true,
                max_age: 0usize,
            },
            extends: false,
            keys: None,
            visible: None,
            is_subscription: false,
            is_node: false,
            rust_typename: create_payload_name.clone(),
        },
        &create_payload_name,
        &create_payload_name,
    );

    // createQuery
    ctx.mutations.push(MetaField {
        name: format!("{}Create", type_name.to_lowercase()),
        description: Some(format!("Create a {}", type_name)),
        args: {
            let mut args = IndexMap::new();
            args.insert(
                "input".to_owned(),
                MetaInputValue {
                    name: "input".to_owned(),
                    description: None,
                    ty: format!("{}!", &create_input_name),
                    default_value: None,
                    visible: None,
                    is_secret: false,
                },
            );
            args
        },
        ty: create_payload_name,
        deprecation: async_graphql::registry::Deprecation::NoDeprecated,
        cache_control: async_graphql::CacheControl {
            public: true,
            max_age: 0usize,
        },
        external: false,
        provides: None,
        requires: None,
        visible: None,
        is_edge: None,
        compute_complexity: None,
        resolve: Some(Resolver {
            id: Some(format!("{}_create_resolver", type_name.to_lowercase())),
            r#type: ResolverType::DynamoMutationResolver(DynamoMutationResolver::CreateNode {
                input: VariableResolveDefinition::InputTypeName("input".to_owned()),
                ty: type_name,
            }),
        }),
        transforms: None,
    });
}

/// Add the remove mutation for a given Object
pub fn add_remove_query<'a>(ctx: &mut VisitorContext<'a>, id_field: &FieldDefinition, type_name: &str) {
    let type_name = type_name.to_string();
    let delete_payload_name = format!("{}DeletePayload", type_name.to_camel());

    // DeletePayload
    ctx.registry.get_mut().create_type(
        &mut |_| MetaType::Object {
            name: delete_payload_name.clone(),
            description: None,
            fields: {
                let mut fields = IndexMap::new();
                let name = "deletedId".to_string();
                fields.insert(
                    name.clone(),
                    MetaField {
                        name,
                        description: None,
                        args: Default::default(),
                        // TODO: Should be infered from the entity depending on the directives
                        ty: "ID!".to_string(),
                        deprecation: Default::default(),
                        cache_control: Default::default(),
                        external: false,
                        requires: None,
                        provides: None,
                        visible: None,
                        compute_complexity: None,
                        is_edge: None,
                        resolve: None,
                        transforms: Some(vec![Transformer::JSONSelect {
                            property: "id".to_string(),
                            functions: vec![],
                        }]),
                    },
                );
                fields
            },
            cache_control: async_graphql::CacheControl {
                public: true,
                max_age: 0usize,
            },
            extends: false,
            keys: None,
            is_node: false,
            visible: None,
            is_subscription: false,
            rust_typename: delete_payload_name.clone(),
        },
        &delete_payload_name,
        &delete_payload_name,
    );

    // deleteMutation
    ctx.mutations.push(MetaField {
        name: format!("{}Delete", type_name.to_lowercase()),
        description: Some(format!("Delete a {} by ID", type_name)),
        args: {
            let mut args = IndexMap::new();
            args.insert(
                "id".to_owned(),
                MetaInputValue {
                    name: "id".to_owned(),
                    description: None,
                    ty: format!("{}!", id_field.ty.node.base),
                    default_value: None,
                    visible: None,
                    is_secret: false,
                },
            );
            args
        },
        ty: delete_payload_name,
        deprecation: async_graphql::registry::Deprecation::NoDeprecated,
        cache_control: async_graphql::CacheControl {
            public: true,
            max_age: 0usize,
        },
        external: false,
        provides: None,
        requires: None,
        visible: None,
        is_edge: None,
        compute_complexity: None,
        resolve: Some(Resolver {
            id: Some(format!("{}_delete_resolver", type_name.to_lowercase())),
            r#type: ResolverType::DynamoMutationResolver(DynamoMutationResolver::DeleteNode {
                id: VariableResolveDefinition::InputTypeName("id".to_owned()),
            }),
        }),
        transforms: None,
    });
}
