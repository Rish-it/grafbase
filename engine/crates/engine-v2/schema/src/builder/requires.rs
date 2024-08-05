use std::collections::BTreeMap;

use crate::{Graph, RequiredField, RequiredFieldId, RequiredFieldSet, RequiredFieldSetId, RequiredFieldSetItem};

use super::{
    coerce::{InputValueCoercer, InputValueError},
    BuildContext, BuildError, SchemaLocation,
};

#[derive(Default)]
pub(super) struct RequiredFieldSetBuffer(Vec<(SchemaLocation, federated_graph::FieldSet)>);

impl RequiredFieldSetBuffer {
    pub(super) fn push(
        &mut self,
        location: SchemaLocation,
        field_set: federated_graph::FieldSet,
    ) -> RequiredFieldSetId {
        let id = RequiredFieldSetId::from(self.0.len());
        self.0.push((location, field_set));
        id
    }

    pub(super) fn try_insert_into(self, ctx: &BuildContext, graph: &mut Graph) -> Result<(), BuildError> {
        let mut input_values = std::mem::take(&mut graph.input_values);
        let mut converter = Converter {
            ctx,
            graph,
            coercer: InputValueCoercer::new(ctx, graph, &mut input_values),
            deduplicated_fields: BTreeMap::new(),
        };

        let mut required_field_sets = Vec::with_capacity(self.0.len());
        for (location, field_set) in self.0 {
            let set =
                converter
                    .convert_set(field_set)
                    .map_err(|err| BuildError::RequiredFieldArgumentCoercionError {
                        location: location.to_string(ctx),
                        err,
                    })?;
            required_field_sets.push(set);
        }

        let mut arguments = converter.deduplicated_fields.into_iter().collect::<Vec<_>>();
        arguments.sort_unstable_by_key(|(_, id)| *id);
        graph.required_fields = arguments.into_iter().map(|(field, _)| field).collect();
        graph.required_field_sets = required_field_sets;
        graph.input_values = input_values;
        Ok(())
    }
}

struct Converter<'a> {
    ctx: &'a BuildContext,
    graph: &'a Graph,
    coercer: InputValueCoercer<'a>,
    deduplicated_fields: BTreeMap<RequiredField, RequiredFieldId>,
}

impl<'a> Converter<'a> {
    fn convert_set(&mut self, field_set: federated_graph::FieldSet) -> Result<RequiredFieldSet, InputValueError> {
        field_set
            .into_iter()
            .filter_map(|item| self.convert_item(item).transpose())
            .collect::<Result<_, _>>()
    }

    fn convert_item(
        &mut self,
        item: federated_graph::FieldSetItem,
    ) -> Result<Option<RequiredFieldSetItem>, InputValueError> {
        let Some(definition_id) = self.ctx.idmaps.field.get(item.field) else {
            return Ok(None);
        };

        let mut federated_arguments = item
            .arguments
            .into_iter()
            .filter_map(|(id, value)| {
                let input_value_definition_id = self.ctx.idmaps.input_value.get(id)?;
                Some((input_value_definition_id, value))
            })
            .collect::<Vec<_>>();
        let mut arguments = Vec::with_capacity(federated_arguments.len());

        for input_value_definition_id in self.graph[definition_id].argument_ids {
            let input_value_definition = &self.graph[input_value_definition_id];
            if let Some(index) = federated_arguments
                .iter()
                .position(|(id, _)| *id == input_value_definition_id)
            {
                let (_, value) = federated_arguments.swap_remove(index);
                let ty = self.graph[input_value_definition_id].ty;
                let input_value_id = self.coercer.coerce(ty, value)?;
                arguments.push((input_value_definition_id, input_value_id));
            } else if let Some(id) = input_value_definition.default_value {
                arguments.push((input_value_definition_id, id));
            } else if input_value_definition.ty.wrapping.is_required() {
                return Err(InputValueError::MissingRequiredArgument(
                    self.ctx.strings[input_value_definition.name].clone(),
                ));
            }
        }

        let field = RequiredField {
            definition_id,
            arguments,
        };

        let n = self.deduplicated_fields.len();
        // Deduplicating arguments allows us to cheaply merge field sets at runtime
        let id = *self
            .deduplicated_fields
            .entry(field)
            .or_insert_with(|| RequiredFieldId::from(n));

        Ok(Some(RequiredFieldSetItem {
            id,
            subselection: self.convert_set(item.subselection)?,
        }))
    }
}
