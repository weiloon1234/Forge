use std::marker::PhantomData;

use super::context::DatatableContext;
use super::value::DatatableValue;

type MappingCallback<M> = Box<dyn Fn(&M, &DatatableContext) -> DatatableValue + Send + Sync>;

/// A computed output-only field for datatable rows.
///
/// Mappings can override existing column values or add new computed fields.
/// They are not automatically sortable or filterable.
pub struct DatatableMapping<M> {
    pub name: String,
    callback: MappingCallback<M>,
    _marker: PhantomData<M>,
}

impl<M> DatatableMapping<M> {
    pub fn new<F>(name: impl Into<String>, callback: F) -> Self
    where
        F: Fn(&M, &DatatableContext) -> DatatableValue + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            callback: Box::new(callback),
            _marker: PhantomData,
        }
    }

    pub fn compute(&self, model: &M, ctx: &DatatableContext) -> DatatableValue {
        (self.callback)(model, ctx)
    }
}
