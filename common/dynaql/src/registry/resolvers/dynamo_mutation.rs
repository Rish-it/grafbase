use super::{ResolvedValue, ResolverContext, ResolverTrait};
use crate::registry::utils::{type_to_base_type, value_to_attribute};
use crate::registry::variables::id::ObfuscatedID;
use crate::registry::variables::VariableResolveDefinition;
use crate::registry::MetaType;
use crate::{Context, Error, ServerError, Value};
use chrono::Utc;
use dynamodb::{
    BatchGetItemLoaderError, DynamoDBBatchersData, DynamoDBContext, QueryKey, TransactionError,
    TxItem,
};
use dynaql_value::Name;
use dynomite::dynamodb::{Delete, Put, TransactWriteItem, Update};
use dynomite::{Attribute, AttributeValue};
use futures_util::future::Shared;
use futures_util::{FutureExt, TryFutureExt};
use indexmap::IndexMap;
use itertools::Itertools;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use ulid_rs::Ulid;

#[non_exhaustive]
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, Hash)]
pub enum DynamoMutationResolver {
    /// Create a new Node
    ///
    /// We do create a new node and store the generated ID into the ResolverContext to allow a
    /// ContextData Resolver to access this id if needed.
    /// When we create a Node with Edges, we fetch those edges before creating the Node and the
    /// vertices.
    ///
    /// # Flow
    ///
    /// -> Generate the ID of the new Node
    /// -> Fetch the Edges needed.
    /// -> Store the Node
    /// -> Store the Vertices.
    ///
    /// # Returns
    ///
    /// This resolver return a Value like this:
    ///
    /// ```json
    /// {
    ///   "id": "<generated_id>"
    /// }
    /// ```
    CreateNode {
        input: VariableResolveDefinition,
        /// Type defined for GraphQL side, it's used to be able to know if we manipulate a Node
        /// and if this Node got Edges. This type must the the Type visible on the GraphQL Schema.
        ty: String,
    },
    /// The delete Node will delete the node and the relation to the associated edges, as
    /// an edge is a Node, we won't have any unreachable node in our Database.
    ///
    /// We also store the deleted ID into the ResolverContext to allow a ContextData Resolver to
    /// access this id if needed.
    ///
    /// # Example
    ///
    /// A node with two edges:
    ///
    /// ```ignore
    ///                     ┌────────┐
    ///                 ┌───┤ Edge 1 │
    ///                 │   └────────┘
    ///      ┌────┐     │
    ///      │Node├─────┤
    ///      └────┘     │
    ///                 │   ┌────────┐
    ///                 └───┤ Edge 2 │
    ///                     └────────┘
    /// ```
    ///
    /// When we delete this node, we'll update the graph to become:
    ///
    /// ```ignore
    ///                     ┌────────┐
    ///                     │ Edge 1 │
    ///                     └────────┘
    ///                     
    ///
    ///
    ///                     ┌────────┐
    ///                     │ Edge 2 │
    ///                     └────────┘
    /// ```
    ///
    /// And as every edges of a Node are a Node too, they are still reachable.
    ///
    /// In the future, when we'll have worked on an async process to optimize we'll be able to
    /// optimize the delete operation:
    ///
    /// In fact it's useless to delete the vertices between the node when you do not have a
    /// bi-directional relaton between nodes. You could only remove the node and have an async
    /// process remove the vertices as soon as possible. It woulnd't affect the future user's
    /// queries but would allow a deletion to be executed with a constant time of one operation.
    ///
    /// # Returns
    ///
    /// This resolver return a Value like this:
    ///
    /// ```json
    /// {
    ///   "id": "<deleted_id>"
    /// }
    /// ```
    DeleteNode { id: VariableResolveDefinition },
    /// Update a Node and related relations
    ///
    /// To update a Node, we need to fetch every duplicate of this node which will
    /// exists linked to other nodes.
    ///
    /// Trigger the update for those basic fields accross every node & duplicate.
    ///
    /// ```json
    /// {
    ///   "id": "<updated_id>"
    /// }
    /// ```
    UpdateNode {
        id: VariableResolveDefinition,
        input: VariableResolveDefinition,
        /// Type defined for GraphQL side, it's used to be able to know if we manipulate a Node
        /// and if this Node got Edges. This type must the the Type visible on the GraphQL Schema.
        ty: String,
    },
}

type SharedSelectionType<'a> = Shared<
    Pin<
        Box<
            dyn Future<
                    Output = Result<
                        HashMap<(String, String), HashMap<String, AttributeValue>>,
                        BatchGetItemLoaderError,
                    >,
                > + Send
                + 'a,
        >,
    >,
>;

type SelectionType<'a> = Pin<
    Box<
        dyn Future<
                Output = Result<
                    HashMap<(String, String), HashMap<String, AttributeValue>>,
                    BatchGetItemLoaderError,
                >,
            > + Send
            + 'a,
    >,
>;

type TransactionType<'a> =
    Pin<Box<dyn Future<Output = Result<ResolvedValue, TransactionError>> + Send + 'a>>;

/// The purpose of this struct is to divide result based on transaction or selection.
/// And these results will be based on a projection of what would exist if we executed
/// the creation and the selection.
///
/// Currently the transaction mechanism we came up with is based on a DataLoader pattern
/// due to DynamoDB inner working which doesn't grant a proper Transaction System
/// where we can begin at the start of the request and at the end if there is an
/// issue, we revert everything.
/// This block will need to be created later.
///
/// As it's implemented following a DataLoader pattern, it's optimized to avoid
/// having too many requests, but it means we should have every modification bach
/// together.
///
/// The idea is to properly work on transaction after the main features for Gateway
/// are done.
struct RecursiveCreation<'a> {
    /// Projected + Actual Selection
    /// If we want to create an Entity with a Edge, the Projected Edge would be
    /// return here for instance, and the real future to create it would be inside
    /// the `transaction`
    pub selection: SharedSelectionType<'a>,
    pub transaction: Vec<TransactionType<'a>>,
}

/// Create an Node and the relation associated to this Node if the Node is
/// modelized
///
/// To create a Node, we'll follow these steps,
/// For every relation:
///     - (Create the sub-node if we need to create it)
///     - Create The relation between the parent-node and the sub-node
/// Return:
///     - The projected data of the sub-node
///
/// For every relation:
///     - If the sub-node should have the reversed relation, create it
///
/// Then create the node
///
/// Return a flattened version of Vec<Future> for every transactions which will
/// need to be run.
fn node_create<'a>(
    ctx: &'a Context<'a>,
    node_ty: &'a MetaType,
    execution_id: Ulid,
    increment: Arc<AtomicUsize>,
    input: IndexMap<Name, Value>,
) -> RecursiveCreation<'a> {
    let current_execution_id = {
        let mut execution_id = Some(execution_id);
        for _ in 0..increment.load(std::sync::atomic::Ordering::SeqCst) {
            execution_id = execution_id.as_ref().and_then(ulid_rs::Ulid::increment);
        }
        execution_id
    }
    .expect("Shouldn't fail");

    let id = format!("{}#{}", node_ty.name(), &current_execution_id);
    // First, to create the Node, we'll need to create the associated relations
    // if they need to be created.
    let relations_to_be_created = node_ty.relations();

    // We do copy every value from the input we do have into the item we'll
    // insert
    let mut item = input
        .clone()
        .into_iter()
        .filter(|(key, _)| !relations_to_be_created.contains_key(key.as_str()))
        .fold(HashMap::new(), |mut acc, (key, val)| {
            let key = key.to_string();
            acc.insert(
                key,
                value_to_attribute(val.into_json().expect("can't fail")),
            );
            acc
        });

    let autogenerated_id_attr = id.clone().into_attr();
    let ty_attr = node_ty.name().to_string().into_attr();
    let now_attr = Utc::now().to_string().into_attr();

    item.insert("__pk".to_string(), autogenerated_id_attr.clone());
    item.insert("__sk".to_string(), autogenerated_id_attr.clone());
    item.insert("__type".to_string(), ty_attr.clone());

    item.insert("created_at".to_string(), now_attr.clone());
    item.insert("updated_at".to_string(), now_attr);

    item.insert("__gsi1pk".to_string(), ty_attr);
    item.insert("__gsi1sk".to_string(), autogenerated_id_attr.clone());

    item.insert("__gsi2pk".to_string(), autogenerated_id_attr.clone());
    item.insert("__gsi2sk".to_string(), autogenerated_id_attr);

    let cloned_item = item.clone();
    let id_cloned = id.clone();
    let selection_future: SelectionType = Box::pin(async move {
        let mut result = HashMap::with_capacity(1);
        result.insert((id_cloned.clone(), id_cloned), cloned_item);
        Ok(result)
    });

    let selection_entity = selection_future.shared();

    // The tricky part here, is, if we run them altogether, it means we'll create
    // edges, but if the mutation fail before the end we could have partial application
    // of the transaction.
    //
    // To solution this issue, we'll split them up in two part, creation, and selection.
    // We'll execute the selections future to check if the selected edges are compliant.
    // Then we'll run the transaction to create every node & relation, if it fails,
    // we'll at least have a rollback splitted by 25, so, even if it dosn't not fix
    // everything, we'll have a way to fix it in the future.
    let (_, mut transactions) = relations_to_be_created
        .into_iter()
        .map(|(field, relation)| {
            (
                &relation.name,
                relation_handle(
                    ctx,
                    node_ty,
                    selection_entity.clone(),
                    field,
                    &relation.name,
                    &input,
                    execution_id,
                    increment.clone(),
                ),
            )
        })
        .fold(
            (Vec::new(), Vec::new()),
            |(mut selections, mut transactions), (relation_name, list_recur)| {
                for curr in list_recur {
                    selections.extend(vec![curr
                        .selection
                        .map_ok(|val| (relation_name.to_owned(), val))]);
                    transactions.extend(curr.transaction.into_iter());
                }
                (selections, transactions)
            },
        );

    // Once we have the edges, either in the process of being created or created
    // we do have their id, so now, we need to:
    //   - Create the targeted Node
    let id_cloned = id.clone();
    let create_future: Pin<
        Box<dyn Future<Output = Result<ResolvedValue, TransactionError>> + Send>,
    > = Box::pin(async move {
        let dynamodb_ctx = ctx.data_unchecked::<DynamoDBContext>();
        let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
        let transaction_batcher = &batchers.transaction;

        // The node is complete now, we'll pack it into a Transaction
        let node_transaction = TxItem {
            pk: id_cloned.clone(),
            sk: id_cloned.clone(),
            relation_name: None,
            transaction: TransactWriteItem {
                put: Some(Put {
                    table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                    item,
                    ..Default::default()
                }),
                ..Default::default()
            },
        };

        transaction_batcher.load_one(node_transaction).await?;

        Ok(ResolvedValue::new(serde_json::json!({
            "id": serde_json::Value::String(id),
        })))
    });

    transactions.extend(vec![create_future]);

    RecursiveCreation {
        selection: selection_entity,
        transaction: transactions,
    }
}

/// Delete a relation on both side if they exist for a relation name for one entity
///
/// The strategy is:
/// -> We get the ID1, ID2
/// So we get, the first relation & the second one
/// Then we remove those
async fn relation_remove<'a>(
    ctx: &'a Context<'a>,
    from: SharedSelectionType<'a>,
    to: String,
    _relation_name: &'a str,
) -> Result<ResolvedValue, TransactionError> {
    let dynamodb_ctx = ctx.data_unchecked::<DynamoDBContext>();
    let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
    let transaction_loader = &batchers.transaction;
    let values = from.await.map_err(|_| TransactionError::UnknownError)?;

    let mut transactions = Vec::with_capacity(values.len() * 2 + 1);

    for ((pk, sk), _) in values
        .into_iter()
        .filter(|((pk, sk), _)| *pk != to || *sk != to)
    {
        let from_to_to = TxItem {
            pk: to.clone(),
            sk: sk.clone(),
            relation_name: None,
            transaction: TransactWriteItem {
                delete: Some(Delete {
                    table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                    key: HashMap::from([
                        ("__pk".to_string(), to.clone().into_attr()),
                        ("__sk".to_string(), sk.into_attr()),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };

        transactions.push(from_to_to);

        let to_to_from = TxItem {
            pk: pk.clone(),
            sk: to.clone(),
            relation_name: None,
            transaction: TransactWriteItem {
                delete: Some(Delete {
                    table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                    key: HashMap::from([
                        ("__pk".to_string(), pk.into_attr()),
                        ("__sk".to_string(), to.clone().into_attr()),
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };

        transactions.push(to_to_from);
    }

    transaction_loader.load_many(transactions).await?;

    Ok(ResolvedValue::new(serde_json::Value::Null))
}

type InputIterRef<'a> = Vec<(&'a Name, &'a Value)>;
type InputIter = Vec<(Name, Value)>;

/// Update a node
///
/// An update means:
///   - Updating basic fields for the entity and also for every duplicate linked
///   to this node.
///   - Create new linked entity if needed
///   - Remove old entity linked if needed
fn node_update<'a>(
    ctx: &'a Context<'a>,
    node_ty: &'a MetaType,
    execution_id: Ulid,
    increment: Arc<AtomicUsize>,
    input: IndexMap<Name, Value>,
    id: String,
) -> RecursiveCreation<'a> {
    let relations = node_ty.relations();

    let id_cloned = id.clone();
    let (_, basic): (InputIterRef<'_>, InputIterRef<'_>) = input
        .iter()
        .partition(|(name, _)| relations.contains_key(name.as_str()));
    let should_update_updated_at = !basic.is_empty();
    // We compute the attribute which will be updated.
    let update_attr: InputIter = basic
        .into_iter()
        .map(|(name, val)| (name.to_owned(), val.to_owned()))
        .collect();

    // We create an updated version of the selected entity
    let selection_updated_future: SelectionType = Box::pin(async move {
        let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
        let loader = &batchers.loader;

        loader
            .load_many(vec![(id_cloned.clone(), id_cloned)])
            .await
            .map(|selected| {
                selected
                    .into_iter()
                    .map(|(id, mut entity)| {
                        for (att_name, att_val) in &update_attr {
                            entity.insert(
                                att_name.to_string(),
                                value_to_attribute(
                                    att_val
                                        .to_owned()
                                        .into_json()
                                        .expect("Shouldn't fail as this is valid json"),
                                ),
                            );
                        }
                        if should_update_updated_at {
                            entity.insert(
                                "updated_at".to_string(),
                                Utc::now().to_string().into_attr(),
                            );
                        }
                        (id, entity)
                    })
                    .collect()
            })
    });
    let selection_entity_updated = selection_updated_future.shared();

    // We manage every relations possible
    let (_, mut transactions) = relations
        .clone()
        .into_iter()
        .map(|(field, relation)| {
            (
                &relation.name,
                relation_handle(
                    ctx,
                    node_ty,
                    selection_entity_updated.clone(),
                    field,
                    &relation.name,
                    &input,
                    execution_id,
                    increment.clone(),
                ),
            )
        })
        .fold(
            (Vec::new(), Vec::new()),
            |(mut selections, mut transactions), (relation_name, list_recur)| {
                for curr in list_recur {
                    selections.extend(vec![(
                        relation_name,
                        curr.selection.map_ok(|val| (relation_name.to_owned(), val)),
                    )]);
                    transactions.extend(curr.transaction.into_iter());
                }
                (selections, transactions)
            },
        );

    // We split the input between basic attribute and relations because we have
    // two separate process based on each of them.
    let (updated_relations, basic): (InputIter, InputIter) = input
        .into_iter()
        .partition(|(name, _)| relations.contains_key(name.as_str()));
    let updated_relations_len = updated_relations.len();

    // We prepare a selection future which will be ran before any transaction
    // We'll execute the update even if the relation will be unlink after.
    // We can optimize this, but it's easier to have the same flow right now.
    let id_cloned = id.clone();
    let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
    let query_loader_reversed = &batchers.query_reversed;
    let select_entities_to_update = query_loader_reversed
        .load_one(QueryKey::new(id, Vec::new()))
        .shared();
    let select_entities_to_update_cloned = select_entities_to_update.clone();

    // We create the update future which will be triggered after every selection future
    // to update the main node and also the replicate.
    // This future will also create/delete relation if needed and create node if needed.
    let update_future: Pin<
        Box<dyn Future<Output = Result<ResolvedValue, TransactionError>> + Send>,
    > = Box::pin(async move {
        let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
        let transaction_batcher = &batchers.transaction;
        let dynamodb_ctx = ctx.data_unchecked::<DynamoDBContext>();
        let items_pk = select_entities_to_update_cloned
            .await
            .map_err(
                #[allow(unused_variables)]
                |err| {
                    #[cfg(feature = "tracing_worker")]
                    logworker::error!(
                        ctx.data_unchecked::<DynamoDBContext>().trace_id,
                        "An error happened while fetching entities {:?}",
                        err
                    );

                    TransactionError::UnknownError
                },
            )?
            .map(|x| x.values)
            .unwrap_or_else(IndexMap::new)
            .into_iter()
            .flat_map(|(_, val)| {
                val.node
                    .into_iter()
                    .chain(val.edges.into_iter().flat_map(|(_, x)| x.into_iter()))
            })
            .filter_map(|mut attrs| attrs.remove("__pk").and_then(|y| y.s));

        let mut exp_values = HashMap::new();

        let mut transactions = Vec::with_capacity(updated_relations_len + 1);

        // We only update non-relation items if there is a need to update them
        if !basic.is_empty() {
            let update_expression = basic
                .into_iter()
                .chain(std::iter::once((
                    Name::new("updated_at"),
                    Value::String(Utc::now().to_string()),
                )))
                .map(|(name, value)| {
                    let new_val = value_to_attribute(value.into_json().unwrap());
                    let idx = format!(":{}", name.as_str());
                    let result = format!("{}={}", name.as_str(), idx);
                    exp_values.insert(idx, new_val);
                    result
                })
                .join(",");

            // For every duplicate we have on this item, we'll need to update it.
            for pk in std::iter::once(id_cloned.clone()).chain(items_pk) {
                let exp = dynomite::attr_map! {
                    "__pk" => pk.clone(),
                    "__sk" => id_cloned.clone(),
                };

                let update_transaction: TransactWriteItem = TransactWriteItem {
                    update: Some(Update {
                        table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                        key: exp,
                        condition_expression: Some(
                            "attribute_exists(#pk) AND attribute_exists(#sk)".to_string(),
                        ),
                        update_expression: format!("set {update_expression}"),
                        expression_attribute_values: Some(exp_values.clone()),
                        expression_attribute_names: Some(HashMap::from([
                            ("#pk".to_string(), "__pk".to_string()),
                            ("#sk".to_string(), "__sk".to_string()),
                        ])),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                let transaction = TxItem {
                    pk,
                    sk: id_cloned.clone(),
                    relation_name: None,
                    transaction: update_transaction,
                };

                transactions.push(transaction);
            }
        }
        transaction_batcher.load_many(transactions).await?;

        Ok(ResolvedValue::new(serde_json::Value::Null))
    });

    transactions.extend(vec![update_future]);

    // We craft a selection future which will run the selection to get entities to
    // update so when the transaction run we prevent any possible race condition.
    let selected: SelectionType = Box::pin(async move {
        let (selection_entity, _) =
            futures_util::join!(selection_entity_updated, select_entities_to_update);

        selection_entity
    });
    let selected_shared = selected.shared();

    RecursiveCreation {
        selection: selected_shared,
        transaction: transactions,
    }
}

/// Get inputs list
fn inputs(parent_input: &Value) -> Option<Vec<&IndexMap<Name, Value>>> {
    match parent_input {
        Value::Object(obj) => Some(vec![obj]),
        Value::List(list) => {
            let input_list = list.iter().map(inputs).flatten().flatten().collect();
            Some(input_list)
        }
        _ => None,
    }
}

async fn create_relation_node<'a>(
    ctx: &'a Context<'a>,
    to_ty: &MetaType,
    parent_value: SharedSelectionType<'a>,
    selected_value: SharedSelectionType<'a>,
    relation_name: &'a str,
) -> Result<ResolvedValue, TransactionError> {
    let dynamodb_ctx = ctx.data_unchecked::<DynamoDBContext>();
    let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
    let transaction_batcher = &batchers.transaction;

    // Reverse Selected -> Parent
    let relation = to_ty
        .relations()
        .into_iter()
        .find(|(_, relation)| relation.name == relation_name);

    match relation {
        Some((_, relation)) => {
            // If we found the relation, it means we'll need a reverse
            // link.
            let relation_attr = AttributeValue {
                ss: Some(vec![relation.name.clone()]),
                ..Default::default()
            };
            let ty_attr = to_ty.name().to_string().into_attr();
            let parent_value = parent_value
                .await
                .map_err(|_| TransactionError::UnknownError)?
                .into_iter()
                .next();

            let selected_type = selected_value
                .clone()
                .await
                .map_err(|_| TransactionError::UnknownError)?
                .into_iter()
                .next();

            if let (Some(((_pk, sk), mut val)), Some(((selected_id, _), _))) =
                (parent_value, selected_type)
            {
                let node_id = selected_id.clone().into_attr();
                val.insert("__gsi1pk".to_string(), ty_attr);
                // We do store the PK into the GSISK to allow us to group edges based on
                // their node.
                // stored.
                val.insert("__gsi1sk".to_string(), node_id.clone());

                // We do replace the PK by the Node's PK.
                val.insert("__pk".to_string(), node_id.clone());
                // The GSI2 is an inversed index, so we update the SK too.
                val.insert("__gsi2sk".to_string(), node_id.clone());

                let mut update_expression = Vec::with_capacity(val.len() + 1);
                let mut expression_attribute_names = HashMap::with_capacity(16);
                let relations_expression = {
                    expression_attribute_names
                        .insert("#relation_name".to_string(), "__relation_name".to_string());
                    Some(("#relation_name", relation_attr))
                };

                let mut exp_values = val
                    .into_iter()
                    .filter(|(name, _)| {
                        name != "__pk" && name != "__sk" && name != "__relation_name"
                    })
                    .map(|(name, val)| {
                        let value_name = format!(":{name}");
                        if let Some(sanitized_name) = name.strip_prefix("__") {
                            let sanitized_name = format!("#{sanitized_name}");
                            update_expression.push(format!("{}={}", &sanitized_name, &value_name));
                            expression_attribute_names.insert(sanitized_name, name);
                        } else {
                            update_expression.push(format!("{}={}", &name, &value_name));
                        }
                        (value_name, val)
                    })
                    .collect::<HashMap<String, AttributeValue>>();

                let relation_str = relations_expression
                    .map(|(x, attr)| {
                        exp_values.insert(":relation_name".to_string(), attr);
                        format!(" ADD {x} :relation_name ")
                    })
                    .unwrap_or_else(String::new);

                let exp = dynomite::attr_map! {
                    "__pk" => selected_id.clone(),
                    "__sk" => sk.clone(),
                };

                let update_expression = update_expression.join(",");

                let tx = TxItem {
                    pk: selected_id.clone(),
                    sk,
                    relation_name: Some(relation.name.clone()),
                    transaction: TransactWriteItem {
                        update: Some(Update {
                            table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                            key: exp,
                            /*
                            condition_expression: Some(
                                "attribute_not_exists(#pk) AND attribute_not_exists(#sk)"
                                    .to_string(),
                            ),
                            */
                            update_expression: format!("set {update_expression} {relation_str}"),
                            expression_attribute_values: Some(exp_values),
                            expression_attribute_names: Some(expression_attribute_names),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                };

                transaction_batcher.load_one(tx).await?;
            };
            Ok(ResolvedValue::new(serde_json::Value::Null))
        }
        _ => Ok(ResolvedValue::new(serde_json::Value::Null)),
    }
}

/// This function will be used into creation / udpate of a relation:
///
/// When we create a new Node, we'll have relations over this node, those relations,
/// depending on the input, will need to create:
///     - The sub-node if it's a Create
///     - The relations between the parent-node and the sub-node
///     - The relations between the sub-node and the parent-node
///
/// When we update a new Node, for a relation it means it can be:
///     - Same as in the creation flow
///     - unlink a Relation which means we'll need to delete the both side of the relation.
///
/// This function will return the Futures which will be used to create those relations
/// and also the Futures which will be used to have the Projected Data
///
/// Recursive function which create a future to either fetch or create an item.
/// So we need to:
/// - Get the sub-input of a type
/// - If we need to link it, we'll only need to fetch it to get the value.
/// - If we do need to create it, and there are sub-types,
///     we'll also need to create the sub-types of this type and the relation
/// - If we do need to create it, and there aren't any sub-types we can trigger
///     the atomic creation.
///
/// # Optimization concern
/// It should be recursive, but not be runned recursively, we want to have every
/// fetch and write optimized into the less affordable number of queries.
///
#[allow(clippy::too_many_arguments)]
fn relation_handle<'a>(
    ctx: &'a Context<'a>,
    parent_ty: &'a MetaType,
    parent_value: SharedSelectionType<'a>,
    relation_field: &'a str,
    relation_name: &'a str,
    input: &IndexMap<Name, Value>,
    execution_id: Ulid,
    increment: Arc<AtomicUsize>,
) -> Vec<RecursiveCreation<'a>> {
    // We determinate the subtype of this relation
    let child_ty: &MetaType = ctx
        .registry()
        .types
        .get(&type_to_base_type(&parent_ty.field_by_name(relation_field).unwrap().ty).unwrap())
        .unwrap();

    // We need to tell if it's a `create` or a `link`
    // So we get the child input first
    let child_input = match input.get(&Name::new(relation_field)).and_then(inputs) {
        Some(val) => val,
        _ => {
            return Vec::new();
        }
    };

    let mut result = Vec::with_capacity(child_input.len());

    for child_input in child_input {
        let create = child_input.get("create");
        let link = child_input.get("link");
        let unlink = child_input.get("unlink");
        let parent_value = parent_value.clone();

        let result_local = match (create, link, unlink) {
            (Some(Value::Object(creation_input)), None, None) => {
                let mut result = node_create(
                    ctx,
                    child_ty,
                    execution_id,
                    increment.clone(),
                    creation_input.clone(),
                );

                let shared_selection_cloned = result.selection.clone();

                let create_normal_future: TransactionType<'a> = Box::pin(create_relation_node(
                    ctx,
                    parent_ty,
                    shared_selection_cloned.clone(),
                    parent_value.clone(),
                    relation_name,
                ));

                let create_reverse_future: TransactionType<'a> = Box::pin(create_relation_node(
                    ctx,
                    child_ty,
                    parent_value,
                    shared_selection_cloned,
                    relation_name,
                ));

                result
                    .transaction
                    .extend(vec![create_normal_future, create_reverse_future]);

                result
            }
            (None, Some(linking_input), None) => {
                // For linking, it's either, Id, Array of Id, or Null
                let field_value = match linking_input {
                    Value::String(inner) => Some(vec![(inner.clone(), inner.clone())]),
                    Value::List(list) => Some(
                        list.iter()
                            .map(|value| match value {
                                Value::String(inner) => (inner.clone(), inner.clone()),
                                _ => panic!(),
                            })
                            .collect(),
                    ),
                    _ => None,
                };

                let selection: SelectionType<'a> = match field_value {
                    Some(field_value) => Box::pin(async move {
                        let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
                        let loader_batcher = &batchers.loader;

                        loader_batcher.load_many(field_value).await
                    }),
                    None => Box::pin(async move { Ok(HashMap::new()) }),
                };
                let shared_selection = selection.shared();

                let create_normal_future: TransactionType<'a> = Box::pin(create_relation_node(
                    ctx,
                    parent_ty,
                    shared_selection.clone(),
                    parent_value.clone(),
                    relation_name,
                ));

                let create_reverse_future: TransactionType<'a> = Box::pin(create_relation_node(
                    ctx,
                    child_ty,
                    parent_value,
                    shared_selection.clone(),
                    relation_name,
                ));

                RecursiveCreation {
                    selection: shared_selection,
                    transaction: vec![create_normal_future, create_reverse_future],
                }
            }
            (None, None, Some(unlinking_input)) => {
                // For unlinking, it's either, Id, Array of Id, or Null
                let field_value = match unlinking_input {
                    Value::String(inner) => Some(vec![(inner.clone(), inner.clone())]),
                    Value::List(list) => Some(
                        list.iter()
                            .map(|value| match value {
                                Value::String(inner) => (inner.clone(), inner.clone()),
                                _ => panic!(),
                            })
                            .collect(),
                    ),
                    _ => None,
                };

                if let Some(field_value) = field_value {
                    let field_value_clone = field_value.clone();
                    let selection: SelectionType<'a> = Box::pin(async move {
                        let batchers = ctx.data_unchecked::<DynamoDBBatchersData>();
                        let loader_batcher = &batchers.loader;

                        loader_batcher.load_many(field_value_clone).await
                    });

                    let shared_selection = selection.shared();

                    let mut transactions = Vec::with_capacity(field_value.len() + 1);

                    for (pk, _) in field_value {
                        let a: TransactionType<'a> = Box::pin(relation_remove(
                            ctx,
                            parent_value.clone(),
                            pk,
                            relation_name,
                        ));
                        transactions.push(a);
                    }

                    RecursiveCreation {
                        selection: shared_selection,
                        transaction: transactions,
                    }
                } else {
                    ctx.add_error(ServerError::new(
                        "If you fill an unlink value it shouldn't be null",
                        Some(ctx.item.pos),
                    ));
                    let selection: SelectionType<'a> = Box::pin(async move { Ok(HashMap::new()) });

                    RecursiveCreation {
                        selection: selection.shared(),
                        transaction: Vec::new(),
                    }
                }
            }
            _ => {
                let selection: SelectionType<'a> = Box::pin(async move { Ok(HashMap::new()) });
                RecursiveCreation {
                    selection: selection.shared(),
                    transaction: Vec::new(),
                }
            }
        };

        result.push(result_local);
    }

    result
}

#[async_trait::async_trait]
impl ResolverTrait for DynamoMutationResolver {
    async fn resolve(
        &self,
        ctx: &Context<'_>,
        resolver_ctx: &ResolverContext<'_>,
        last_resolver_value: Option<&ResolvedValue>,
    ) -> Result<ResolvedValue, Error> {
        let batchers = &ctx.data::<DynamoDBBatchersData>()?;
        let transaction_batcher = &batchers.transaction;
        let dynamodb_ctx = ctx.data::<DynamoDBContext>()?;

        match self {
            // This one is tricky, when we create a new node, we have to check that the node do not
            // contains any Edges on the first level. If there is an edge at the first level we
            // need to fetch this edge as a node and store it alongside the actual node.
            //
            // Why?
            //
            // Because it's how we store the data.
            DynamoMutationResolver::CreateNode { input, ty } => {
                let ctx_ty = ctx.registry().types.get(ty).ok_or_else(|| {
                    Error::new("Internal Error: Failed process the associated schema.")
                })?;

                let autogenerated_id = format!("{}#{}", ty, resolver_ctx.execution_id);

                let input = match input
                    .param(ctx, last_resolver_value.map(|x| x.data_resolved.borrow()))?
                    .expect("can't fail")
                {
                    Value::Object(inner) => inner,
                    _ => {
                        return Err(Error::new("Internal Error: failed to infer key"));
                    }
                };

                let creation = node_create(
                    ctx,
                    ctx_ty,
                    resolver_ctx.execution_id.to_owned(),
                    Arc::new(AtomicUsize::new(0)),
                    input,
                );
                let _ = creation.selection.await?;
                let _ = futures_util::future::try_join_all(creation.transaction).await?;

                Ok(ResolvedValue::new(serde_json::json!({
                    "id": serde_json::Value::String(autogenerated_id),
                })))
            }
            DynamoMutationResolver::UpdateNode { id, input, ty } => {
                let ctx_ty = ctx.registry().types.get(ty).ok_or_else(|| {
                    Error::new("Internal Error: Failed process the associated schema.")
                })?;

                let id =
                    id.expect_string(ctx, last_resolver_value.map(|x| x.data_resolved.borrow()))?;

                ObfuscatedID::expect(&id, &ty)
                    .map_err(|err| err.into_server_error(ctx.item.pos))?;

                let input =
                    input.expect_obj(ctx, last_resolver_value.map(|x| x.data_resolved.borrow()))?;

                let update = node_update(
                    ctx,
                    ctx_ty,
                    resolver_ctx.execution_id.to_owned(),
                    Arc::new(AtomicUsize::new(0)),
                    input,
                    id.clone(),
                );

                let _ = update.selection.await?;
                let _ = futures_util::future::try_join_all(update.transaction).await?;

                Ok(ResolvedValue::new(serde_json::json!({
                    "id": serde_json::Value::String(id),
                })))
            }
            DynamoMutationResolver::DeleteNode { id } => {
                let query_loader = &batchers.query;
                let query_loader_reversed = &batchers.query_reversed;

                let id_to_be_deleted = match id
                    .param(ctx, last_resolver_value.map(|x| x.data_resolved.borrow()))?
                    .expect("can't fail")
                {
                    Value::String(inner) => inner,
                    _ => {
                        return Err(Error::new("Internal Error: failed to infer key"));
                    }
                };

                let items_pk =
                    query_loader.load_one(QueryKey::new(id_to_be_deleted.clone(), Vec::new()));

                let items_sk = query_loader_reversed
                    .load_one(QueryKey::new(id_to_be_deleted.clone(), Vec::new()));

                let async_fetch_entities = vec![items_pk, items_sk];

                let items_to_be_deleted: Vec<TxItem> = futures_util::future::try_join_all(async_fetch_entities)
                    .await?
                    .into_iter()
                    .flatten()
                    .flat_map(|x| x.values.into_iter().flat_map(|(_, y)| y.node.into_iter().chain(y.edges.into_iter().flat_map(|(_, val)| val.into_iter()))))
                    .filter_map(|val| {
                        let pk = match val.get("__pk").and_then(|x| x.s.clone()) {
                            Some(value) => value,
                            None => {
                                ctx.add_error(ServerError::new("Internal Error: An issue happened while handeling some database data.", Some(ctx.item.pos)));
                                #[cfg(feature = "tracing_worker")]
                                logworker::error!(dynamodb_ctx.trace_id, "An issue happened on the database while removing an item. Table: {}, id: {}", dynamodb_ctx.dynamodb_table_name, id_to_be_deleted);
                                return None;
                            }
                        };

                        let sk = match val.get("__sk").and_then(|x| x.s.clone()) {
                            Some(value) => value,
                            None => {
                                ctx.add_error(ServerError::new("Internal Error: An issue happened while handeling some database data.", Some(ctx.item.pos)));
                                #[cfg(feature = "tracing_worker")]
                                logworker::error!(dynamodb_ctx.trace_id, "An issue happened on the database while removing an item. Table: {}, id: {}", dynamodb_ctx.dynamodb_table_name, id_to_be_deleted);
                                return None;
                            }
                        };

                        let new_item = val
                            .into_iter()
                            .filter(|(key, _)| key == "__pk" || key == "__sk")
                            .collect();

                        Some(TxItem {
                                pk,
                                sk,
                                relation_name: None,
                                transaction: TransactWriteItem {
                                    delete: Some(Delete {
                                        table_name: dynamodb_ctx.dynamodb_table_name.clone(),
                                        key: new_item,
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                            })
                    }).collect();

                if items_to_be_deleted.is_empty() {
                    return Err(Error::new(
                        "This item was not found, you can't delete an inexistant item.",
                    ));
                }

                transaction_batcher.load_many(items_to_be_deleted).await?;

                Ok(ResolvedValue::new(serde_json::json!({
                    "id": serde_json::Value::String(id_to_be_deleted),
                })))
            }
        }
    }
}
