use std::marker::PhantomData;

use crate::database::{Column, OrderDirection};

/// A default sort declaration for a datatable column.
///
/// Constructed from typed `Column<M, T>` references.
pub struct DatatableSort<M> {
    pub column_name: String,
    pub direction: OrderDirection,
    _marker: PhantomData<M>,
}

impl<M> DatatableSort<M> {
    pub fn asc<T>(column: Column<M, T>) -> Self
    where
        M: 'static,
    {
        Self {
            column_name: column.name().to_string(),
            direction: OrderDirection::Asc,
            _marker: PhantomData,
        }
    }

    pub fn desc<T>(column: Column<M, T>) -> Self
    where
        M: 'static,
    {
        Self {
            column_name: column.name().to_string(),
            direction: OrderDirection::Desc,
            _marker: PhantomData,
        }
    }
}
