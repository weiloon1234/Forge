use std::collections::HashMap;

use crate::database::{DbValue, Model, QueryExecutor};
use crate::foundation::Result;

/// Trait for defining model factories with default values for testing.
///
/// ```ignore
/// impl Factory for User {
///     fn definition() -> Vec<(&'static str, DbValue)> {
///         vec![
///             ("email", DbValue::Text(format!("user-{}@test.com", Token::hex(4).unwrap()))),
///             ("name", DbValue::Text("Test User".to_string())),
///             ("active", DbValue::Bool(true)),
///         ]
///     }
/// }
/// ```
pub trait Factory: Model {
    /// Define default column values for this model.
    fn definition() -> Vec<(&'static str, DbValue)>;
}

/// Builder for creating model instances from factory definitions.
pub struct FactoryBuilder<M: Factory> {
    overrides: HashMap<String, DbValue>,
    count: usize,
    _phantom: std::marker::PhantomData<M>,
}

impl<M: Factory> FactoryBuilder<M> {
    pub fn new() -> Self {
        Self {
            overrides: HashMap::new(),
            count: 1,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Override a specific column value.
    pub fn set(mut self, column: &str, value: impl Into<DbValue>) -> Self {
        self.overrides.insert(column.to_string(), value.into());
        self
    }

    /// Create multiple instances.
    pub fn count(mut self, n: usize) -> Self {
        self.count = n;
        self
    }

    /// Build the final column values (defaults merged with overrides).
    fn build_values(&self) -> Vec<(String, DbValue)> {
        let defaults = M::definition();
        let mut values: Vec<(String, DbValue)> = defaults
            .into_iter()
            .map(|(k, v)| {
                if let Some(override_val) = self.overrides.get(k) {
                    (k.to_string(), override_val.clone())
                } else {
                    (k.to_string(), v)
                }
            })
            .collect();

        // Add any overrides that aren't in defaults
        for (key, value) in &self.overrides {
            if !values.iter().any(|(k, _)| k == key) {
                values.push((key.clone(), value.clone()));
            }
        }

        values
    }

    /// Insert one or more records into the database and return them.
    pub async fn create<E>(&self, executor: &E) -> Result<Vec<M>>
    where
        E: QueryExecutor,
    {
        let table = M::table_meta();
        let mut results = Vec::with_capacity(self.count);

        for _ in 0..self.count {
            let values = self.build_values();
            let col_names: Vec<String> = values.iter().map(|(k, _)| format!("\"{k}\"")).collect();
            let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("${i}")).collect();

            let sql = format!(
                "INSERT INTO \"{}\" ({}) VALUES ({}) RETURNING *",
                table.name(),
                col_names.join(", "),
                placeholders.join(", "),
            );

            let bindings: Vec<DbValue> = values.into_iter().map(|(_, v)| v).collect();

            let rows = executor.raw_query(&sql, &bindings).await?;
            if let Some(row) = rows.into_iter().next() {
                let model = table.hydrate_record(&row)?;
                results.push(model);
            }
        }

        Ok(results)
    }

    /// Insert a single record and return it.
    pub async fn create_one<E>(&self, executor: &E) -> Result<M>
    where
        E: QueryExecutor,
    {
        let mut results = self.create(executor).await?;
        results
            .pop()
            .ok_or_else(|| crate::foundation::Error::message("factory create returned no rows"))
    }
}

impl<M: Factory> Default for FactoryBuilder<M> {
    fn default() -> Self {
        Self::new()
    }
}
