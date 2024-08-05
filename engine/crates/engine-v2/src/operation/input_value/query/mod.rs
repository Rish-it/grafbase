mod de;
mod ser;
mod view;

use id_derives::{Id, IndexImpls};
use id_newtypes::IdRange;
use schema::{EnumValueId, InputValue, InputValueDefinitionId, InputValueSet, SchemaInputValue, SchemaInputValueId};

use crate::operation::{OperationWalker, PreparedOperationWalker, VariableDefinitionId};

pub(crate) use view::*;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize, IndexImpls)]
pub(crate) struct QueryInputValues {
    /// Individual input values and list values
    #[indexed_by(QueryInputValueId)]
    values: Vec<QueryInputValue>,

    /// InputObject's fields
    #[indexed_by(QueryInputObjectFieldValueId)]
    input_fields: Vec<(InputValueDefinitionId, QueryInputValue)>,

    /// Object's fields (for JSON)
    #[indexed_by(QueryInputKeyValueId)]
    key_values: Vec<(String, QueryInputValue)>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, serde::Serialize, serde::Deserialize, Id)]
pub struct QueryInputValueId(std::num::NonZero<u32>);

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, serde::Serialize, serde::Deserialize, Id)]
pub struct QueryInputObjectFieldValueId(std::num::NonZero<u32>);

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, serde::Serialize, serde::Deserialize, Id)]
pub struct QueryInputKeyValueId(std::num::NonZero<u32>);

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum QueryInputValue {
    #[default]
    Null,
    String(String),
    EnumValue(EnumValueId),
    Int(i32),
    BigInt(i64),
    Float(f64),
    Boolean(bool),
    InputObject(IdRange<QueryInputObjectFieldValueId>),
    List(IdRange<QueryInputValueId>),

    /// for JSON
    Map(IdRange<QueryInputKeyValueId>),
    U64(u64),

    DefaultValue(SchemaInputValueId),
    Variable(VariableDefinitionId),
}

impl QueryInputValues {
    pub fn push_value(&mut self, value: QueryInputValue) -> QueryInputValueId {
        let id = QueryInputValueId::from(self.values.len());
        self.values.push(value);
        id
    }

    /// Reserve InputValue slots for a list, avoiding the need for an intermediate
    /// Vec to hold values as we need them to be contiguous.
    pub fn reserve_list(&mut self, n: usize) -> IdRange<QueryInputValueId> {
        let start = self.values.len();
        self.values.reserve(n);
        for _ in 0..n {
            self.values.push(QueryInputValue::Null);
        }
        (start..self.values.len()).into()
    }
    /// Reserve InputKeyValue slots for a map, avoiding the need for an intermediate
    /// Vec to hold values as we need them to be contiguous.
    pub fn reserve_map(&mut self, n: usize) -> IdRange<QueryInputKeyValueId> {
        let start = self.key_values.len();
        self.key_values.reserve(n);
        for _ in 0..n {
            self.key_values.push((String::new(), QueryInputValue::Null));
        }
        (start..self.key_values.len()).into()
    }

    pub fn append_input_object(
        &mut self,
        fields: &mut Vec<(InputValueDefinitionId, QueryInputValue)>,
    ) -> IdRange<QueryInputObjectFieldValueId> {
        let start = self.input_fields.len();
        self.input_fields.append(fields);
        (start..self.input_fields.len()).into()
    }
}

pub(crate) type QueryInputValueWalker<'a> = PreparedOperationWalker<'a, &'a QueryInputValue, ()>;

impl<'a> QueryInputValueWalker<'a> {
    pub fn is_undefined(&self) -> bool {
        match self.item {
            QueryInputValue::Variable(id) => self.walk(*id).as_value().is_undefined(),
            _ => false,
        }
    }

    /// Used for GraphQL query generation to only include values in the query string that would be
    /// present after query normalization.
    pub fn to_normalized_query_const_value_str(self) -> Option<&'a str> {
        Some(match self.item {
            QueryInputValue::EnumValue(id) => self.schema_walker.walk(*id).name(),
            QueryInputValue::Boolean(b) => {
                if *b {
                    "true"
                } else {
                    "false"
                }
            }
            QueryInputValue::DefaultValue(id) => match &self.schema_walker.as_ref()[*id] {
                SchemaInputValue::EnumValue(id) => self.schema_walker.walk(*id).name(),
                SchemaInputValue::Boolean(b) => {
                    if *b {
                        "true"
                    } else {
                        "false"
                    }
                }
                _ => return None,
            },
            _ => return None,
        })
    }

    pub fn with_selection_set(self, selection_set: &'a InputValueSet) -> QueryInputValueView<'a> {
        QueryInputValueView {
            inner: self,
            selection_set,
        }
    }
}

impl<'a> From<QueryInputValueWalker<'a>> for InputValue<'a> {
    fn from(walker: QueryInputValueWalker<'a>) -> Self {
        let input_values = &walker.operation.query_input_values;
        match walker.item {
            QueryInputValue::Null => InputValue::Null,
            QueryInputValue::String(s) => InputValue::String(s.as_str()),
            QueryInputValue::EnumValue(id) => InputValue::EnumValue(*id),
            QueryInputValue::Int(n) => InputValue::Int(*n),
            QueryInputValue::BigInt(n) => InputValue::BigInt(*n),
            QueryInputValue::Float(f) => InputValue::Float(*f),
            QueryInputValue::Boolean(b) => InputValue::Boolean(*b),
            QueryInputValue::InputObject(ids) => {
                let mut fields = Vec::with_capacity(ids.len());
                for (definition_id, value) in &input_values[*ids] {
                    let value = walker.walk(value);
                    // https://spec.graphql.org/October2021/#sec-Input-Objects.Input-Coercion
                    if !value.is_undefined() {
                        fields.push((*definition_id, value.into()));
                    }
                }
                InputValue::InputObject(fields.into_boxed_slice())
            }
            QueryInputValue::List(ids) => {
                let mut values = Vec::with_capacity(ids.len());
                for id in *ids {
                    values.push(walker.walk(&input_values[id]).into());
                }
                InputValue::List(values.into_boxed_slice())
            }
            QueryInputValue::Map(ids) => {
                let mut key_values = Vec::with_capacity(ids.len());
                for (key, value) in &input_values[*ids] {
                    let value = walker.walk(value);
                    key_values.push((key.as_ref(), value.into()));
                }
                InputValue::Map(key_values.into_boxed_slice())
            }
            QueryInputValue::U64(n) => InputValue::U64(*n),
            QueryInputValue::DefaultValue(id) => walker.schema_walker.walk(&walker.schema_walker.as_ref()[*id]).into(),
            QueryInputValue::Variable(id) => walker.walk(*id).as_value().to_input_value().unwrap_or_default(),
        }
    }
}

impl PartialEq<SchemaInputValue> for OperationWalker<'_, &QueryInputValue, ()> {
    fn eq(&self, other: &SchemaInputValue) -> bool {
        let input_values = &self.operation.query_input_values;
        match (self.item, other) {
            (QueryInputValue::Null, SchemaInputValue::Null) => true,
            (QueryInputValue::String(l), SchemaInputValue::String(r)) => l == &self.schema_walker[*r],
            (QueryInputValue::EnumValue(l), SchemaInputValue::EnumValue(r)) => l == r,
            (QueryInputValue::Int(l), SchemaInputValue::Int(r)) => l == r,
            (QueryInputValue::BigInt(l), SchemaInputValue::BigInt(r)) => l == r,
            (QueryInputValue::U64(l), SchemaInputValue::U64(r)) => l == r,
            (QueryInputValue::Float(l), SchemaInputValue::Float(r)) => l == r,
            (QueryInputValue::Boolean(l), SchemaInputValue::Boolean(r)) => l == r,
            (QueryInputValue::InputObject(lids), SchemaInputValue::InputObject(rids)) => {
                let op_input_values = &input_values[*lids];
                let schema_input_values = &self.schema_walker.as_ref()[*rids];

                if op_input_values.len() != schema_input_values.len() {
                    return false;
                }

                for (id, input_value) in op_input_values {
                    let input_value = self.walk(input_value);
                    if let Ok(i) = schema_input_values.binary_search_by(|probe| probe.0.cmp(id)) {
                        if !input_value.eq(&schema_input_values[i].1) {
                            return false;
                        }
                    } else {
                        return false;
                    };
                }

                true
            }
            (QueryInputValue::List(lids), SchemaInputValue::List(rids)) => {
                let left = &input_values[*lids];
                let right = &self.schema_walker.as_ref()[*rids];
                if left.len() != right.len() {
                    return false;
                }
                for (left_value, right_value) in left.iter().zip(right) {
                    if !self.walk(left_value).eq(right_value) {
                        return false;
                    }
                }
                true
            }
            (QueryInputValue::Map(ids), SchemaInputValue::Map(other_ids)) => {
                let op_kv = &input_values[*ids];
                let schema_kv = &self.schema_walker[*other_ids];

                for (key, value) in op_kv {
                    let value = self.walk(value);
                    if let Ok(i) = schema_kv.binary_search_by(|probe| self.schema_walker[probe.0].cmp(key)) {
                        if !value.eq(&schema_kv[i].1) {
                            return false;
                        }
                    } else {
                        return false;
                    };
                }

                true
            }
            (QueryInputValue::DefaultValue(id), value) => self
                .schema_walker
                .walk(&self.schema_walker.as_ref()[*id])
                .eq(&self.schema_walker.walk(value)),
            (QueryInputValue::Variable(_), _) => false,
            // A bit tedious, but avoids missing a case
            (QueryInputValue::Null, _) => false,
            (QueryInputValue::String(_), _) => false,
            (QueryInputValue::EnumValue(_), _) => false,
            (QueryInputValue::Int(_), _) => false,
            (QueryInputValue::BigInt(_), _) => false,
            (QueryInputValue::U64(_), _) => false,
            (QueryInputValue::Float(_), _) => false,
            (QueryInputValue::Boolean(_), _) => false,
            (QueryInputValue::InputObject(_), _) => false,
            (QueryInputValue::List(_), _) => false,
            (QueryInputValue::Map(_), _) => false,
        }
    }
}

impl std::fmt::Debug for QueryInputValueWalker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let input_values = &self.operation.query_input_values;
        match self.item {
            QueryInputValue::Null => write!(f, "Null"),
            QueryInputValue::String(s) => s.fmt(f),
            QueryInputValue::EnumValue(id) => f
                .debug_tuple("EnumValue")
                .field(&self.schema_walker.walk(*id).name())
                .finish(),
            QueryInputValue::Int(n) => f.debug_tuple("Int").field(n).finish(),
            QueryInputValue::BigInt(n) => f.debug_tuple("BigInt").field(n).finish(),
            QueryInputValue::U64(n) => f.debug_tuple("U64").field(n).finish(),
            QueryInputValue::Float(n) => f.debug_tuple("Float").field(n).finish(),
            QueryInputValue::Boolean(b) => b.fmt(f),
            QueryInputValue::InputObject(ids) => {
                let mut map = f.debug_struct("InputObject");
                for (input_value_definition_id, value) in &input_values[*ids] {
                    map.field(
                        self.schema_walker.walk(*input_value_definition_id).name(),
                        &self.walk(value),
                    );
                }
                map.finish()
            }
            QueryInputValue::List(ids) => {
                let mut seq = f.debug_list();
                for value in &input_values[*ids] {
                    seq.entry(&self.walk(value));
                }
                seq.finish()
            }
            QueryInputValue::Map(ids) => {
                let mut map = f.debug_map();
                for (key, value) in &input_values[*ids] {
                    map.entry(&key, &self.walk(value));
                }
                map.finish()
            }
            QueryInputValue::DefaultValue(id) => f
                .debug_tuple("DefaultValue")
                .field(&self.schema_walker.walk(&self.schema_walker.as_ref()[*id]))
                .finish(),
            QueryInputValue::Variable(id) => f.debug_tuple("Variable").field(&self.walk(*id)).finish(),
        }
    }
}
