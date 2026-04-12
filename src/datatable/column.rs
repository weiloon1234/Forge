use std::marker::PhantomData;

use crate::database::{Column, DbType};

/// A datatable column descriptor.
///
/// Stores column metadata for rendering, filtering, sorting, and export.
/// Constructed from typed `Column<M, T>` references but stores the column name
/// as a `String` so heterogeneous columns can live in the same `Vec`.
pub struct DatatableColumn<M> {
    pub name: String,
    pub label: String,
    pub sortable: bool,
    pub filterable: bool,
    pub exportable: bool,
    pub relation: Option<String>,
    db_type: DbType,
    _marker: PhantomData<M>,
}

impl<M> DatatableColumn<M>
where
    M: 'static,
{
    pub fn field<T>(column: Column<M, T>) -> Self {
        Self {
            name: column.name().to_string(),
            label: column.name().to_string(),
            sortable: false,
            filterable: false,
            exportable: false,
            relation: None,
            db_type: column.db_type(),
            _marker: PhantomData,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    pub fn filterable(mut self) -> Self {
        self.filterable = true;
        self
    }

    pub fn exportable(mut self) -> Self {
        self.exportable = true;
        self
    }

    pub fn relation(mut self, relation: impl Into<String>) -> Self {
        self.relation = Some(relation.into());
        self
    }

    pub fn db_type(&self) -> DbType {
        self.db_type
    }
}
