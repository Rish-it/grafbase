use id_newtypes::{IdRange, IdRangeIterator};
use schema::{InputValueDefinitionId, InputValueDefinitionWalker, InputValueSerdeError, InputValueSet};
use serde::{de::value::MapDeserializer, forward_to_deserialize_any};

use crate::operation::{FieldArgumentId, QueryInputValueWalker};

mod view;

pub(crate) use view::*;

use super::PreparedOperationWalker;

pub type FieldArgumentWalker<'a> = PreparedOperationWalker<'a, FieldArgumentId, InputValueDefinitionId>;

impl<'a> FieldArgumentWalker<'a> {
    pub fn value(&self) -> Option<QueryInputValueWalker<'a>> {
        let value = self.walk_with(&self.operation.query_input_values[self.as_ref().input_value_id], ());
        if value.is_undefined() {
            None
        } else {
            Some(value)
        }
    }
}

impl<'a> std::ops::Deref for FieldArgumentWalker<'a> {
    type Target = InputValueDefinitionWalker<'a>;

    fn deref(&self) -> &Self::Target {
        &self.schema_walker
    }
}

impl std::fmt::Debug for FieldArgumentWalker<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldArgumentWalker")
            .field("name", &self.name())
            .field("value", &self.value())
            .finish()
    }
}

pub type FieldArgumentsWalker<'a> = PreparedOperationWalker<'a, IdRange<FieldArgumentId>, ()>;

impl<'a> FieldArgumentsWalker<'a> {
    pub fn is_empty(&self) -> bool {
        self.item.is_empty()
    }

    pub fn with_selection_set(self, selection_set: &'a InputValueSet) -> FieldArgumentsView<'a> {
        FieldArgumentsView {
            inner: self,
            selection_set,
        }
    }
}

impl<'a> IntoIterator for FieldArgumentsWalker<'a> {
    type Item = FieldArgumentWalker<'a>;

    type IntoIter = FieldArgumentsIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        FieldArgumentsIterator(self.walk(self.item.into_iter()))
    }
}

pub(crate) struct FieldArgumentsIterator<'a>(PreparedOperationWalker<'a, IdRangeIterator<FieldArgumentId>, ()>);

impl<'a> Iterator for FieldArgumentsIterator<'a> {
    type Item = FieldArgumentWalker<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .item
            .next()
            .map(|id| self.0.walk_with(id, self.0.operation[id].input_value_definition_id))
    }
}

impl ExactSizeIterator for FieldArgumentsIterator<'_> {
    fn len(&self) -> usize {
        self.0.item.len()
    }
}

impl<'de> serde::Deserializer<'de> for FieldArgumentsWalker<'de> {
    type Error = InputValueSerdeError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        MapDeserializer::new(self.into_iter().filter_map(|arg| {
            let value = arg.value()?;
            Some((arg.name(), value))
        }))
        .deserialize_any(visitor)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier option ignored_any
    }
}
