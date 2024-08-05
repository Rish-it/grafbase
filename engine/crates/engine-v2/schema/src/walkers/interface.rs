use super::{FieldDefinitionWalker, SchemaWalker};
use crate::{InterfaceDefinitionId, ObjectDefinitionWalker, TypeSystemDirectivesWalker};

pub type InterfaceDefinitionWalker<'a> = SchemaWalker<'a, InterfaceDefinitionId>;

impl<'a> InterfaceDefinitionWalker<'a> {
    pub fn name(&self) -> &'a str {
        self.names.interface(self.schema, self.item)
    }

    pub fn fields(self) -> impl Iterator<Item = FieldDefinitionWalker<'a>> + 'a {
        let fields = self.schema[self.item].fields;
        fields.into_iter().map(move |field_id| self.walk(field_id))
    }

    pub fn interfaces(self) -> impl ExactSizeIterator<Item = InterfaceDefinitionWalker<'a>> + 'a {
        self.as_ref()
            .interfaces
            .clone()
            .into_iter()
            .map(move |id| self.walk(id))
    }

    pub fn possible_types(self) -> impl ExactSizeIterator<Item = ObjectDefinitionWalker<'a>> + 'a {
        self.as_ref()
            .possible_types
            .clone()
            .into_iter()
            .map(move |id| self.walk(id))
    }

    pub fn directives(&self) -> TypeSystemDirectivesWalker<'a> {
        self.walk(self.as_ref().directives)
    }
}

impl<'a> std::fmt::Debug for InterfaceDefinitionWalker<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Interface")
            .field("id", &usize::from(self.item))
            .field("name", &self.name())
            .field("fields", &self.fields().map(|f| f.name()).collect::<Vec<_>>())
            .field("interfaces", &self.interfaces().map(|i| i.name()).collect::<Vec<_>>())
            .field(
                "possible_types",
                &self.possible_types().map(|t| t.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}
