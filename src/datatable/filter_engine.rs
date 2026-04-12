use crate::database::{
    ColumnRef, ComparisonOp, Condition, DbType, DbValue, Expr, Model, ModelQuery, OrderBy,
};
use crate::foundation::{Error, Result};
use crate::support::{Date, DateTime, LocalDateTime};

use super::column::DatatableColumn;
use super::request::{DatatableFilterInput, DatatableFilterOp, DatatableFilterValue, DatatableSortInput};

// ---------------------------------------------------------------------------
// Auto-filter application
// ---------------------------------------------------------------------------

/// Apply structured filter inputs to a model query.
///
/// Only columns declared as `filterable` participate. Each filter generates
/// the appropriate `Condition` using runtime `ColumnRef` construction.
pub fn apply_auto_filters<M: Model>(
    mut query: ModelQuery<M>,
    filters: &[DatatableFilterInput],
    columns: &[DatatableColumn<M>],
    table_name: &str,
) -> Result<ModelQuery<M>> {
    for filter in filters {
        // LikeAny is special — pipe-delimited field list
        if filter.op == DatatableFilterOp::LikeAny {
            query = apply_like_any(query, filter, columns, table_name)?;
            continue;
        }

        // Find the declared column
        let col = match columns.iter().find(|c| c.name == filter.field) {
            Some(c) if c.filterable => c,
            Some(_) => {
                return Err(Error::message(format!(
                    "column '{}' is not filterable",
                    filter.field
                )));
            }
            None => {
                return Err(Error::message(format!(
                    "unknown filter field '{}'",
                    filter.field
                )));
            }
        };

        let col_ref = ColumnRef::new(table_name, &col.name).typed(col.db_type());
        let col_expr = Expr::column(col_ref);

        let condition = build_filter_condition(&filter.op, col_expr, &filter.value, col.db_type())?;
        query = query.where_(condition);
    }

    Ok(query)
}

fn apply_like_any<M: Model>(
    query: ModelQuery<M>,
    filter: &DatatableFilterInput,
    columns: &[DatatableColumn<M>],
    table_name: &str,
) -> Result<ModelQuery<M>> {
    let text = match &filter.value {
        DatatableFilterValue::Text(s) => s.clone(),
        _ => return Err(Error::message("LikeAny requires a text value")),
    };

    let pattern = format!("%{text}%");
    let field_names: Vec<&str> = filter.field.split('|').collect();

    let mut conditions = Vec::new();
    for name in &field_names {
        let col = match columns.iter().find(|c| c.name == *name) {
            Some(c) if c.filterable => c,
            _ => continue,
        };
        let col_ref = ColumnRef::new(table_name, &col.name).typed(col.db_type());
        conditions.push(Condition::compare(
            Expr::column(col_ref),
            ComparisonOp::Like,
            Expr::value(DbValue::Text(pattern.clone())),
        ));
    }

    if conditions.is_empty() {
        return Ok(query);
    }

    Ok(query.where_(Condition::or(conditions)))
}

fn build_filter_condition(
    op: &DatatableFilterOp,
    col_expr: Expr,
    value: &DatatableFilterValue,
    db_type: DbType,
) -> Result<Condition> {
    match op {
        DatatableFilterOp::Eq => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(col_expr, ComparisonOp::Eq, Expr::value(db_val)))
        }
        DatatableFilterOp::NotEq => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::NotEq,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::Like => {
            let text = expect_text(value)?;
            let pattern = format!("%{text}%");
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Like,
                Expr::value(DbValue::Text(pattern)),
            ))
        }
        DatatableFilterOp::Gt => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(col_expr, ComparisonOp::Gt, Expr::value(db_val)))
        }
        DatatableFilterOp::Gte => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Gte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::Lt => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(col_expr, ComparisonOp::Lt, Expr::value(db_val)))
        }
        DatatableFilterOp::Lte => {
            let db_val = filter_value_to_db(value, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Lte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::In => {
            let values = expect_values(value)?;
            let db_values: Vec<DbValue> = values
                .iter()
                .map(|v| text_to_db_value(v, db_type))
                .collect::<Result<Vec<_>>>()?;
            Ok(Condition::InList {
                expr: col_expr,
                values: db_values,
            })
        }
        DatatableFilterOp::Date => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(col_expr, ComparisonOp::Eq, Expr::value(db_val)))
        }
        DatatableFilterOp::DateFrom => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Gte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::DateTo => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Lte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::Datetime => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(col_expr, ComparisonOp::Eq, Expr::value(db_val)))
        }
        DatatableFilterOp::DatetimeFrom => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Gte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::DatetimeTo => {
            let text = expect_text(value)?;
            let db_val = text_to_db_value(&text, db_type)?;
            Ok(Condition::compare(
                col_expr,
                ComparisonOp::Lte,
                Expr::value(db_val),
            ))
        }
        DatatableFilterOp::Has => {
            let col_ref = match col_expr {
                Expr::Column(col_ref) => col_ref,
                _ => unreachable!("column expression should be ColumnRef"),
            };
            Ok(Condition::IsNotNull(col_ref))
        }
        DatatableFilterOp::HasLike => {
            let text = expect_text(value)?;
            let pattern = format!("%{text}%");
            let col_ref = match col_expr {
                Expr::Column(col_ref) => col_ref,
                _ => unreachable!("column expression should be ColumnRef"),
            };
            let not_null = Condition::IsNotNull(col_ref.clone());
            let like = Condition::compare(
                Expr::column(col_ref),
                ComparisonOp::Like,
                Expr::value(DbValue::Text(pattern)),
            );
            Ok(Condition::and(vec![not_null, like]))
        }
        DatatableFilterOp::LikeAny => {
            // Handled separately in apply_like_any
            Err(Error::message("LikeAny should be handled separately"))
        }
    }
}

// ---------------------------------------------------------------------------
// Sort application
// ---------------------------------------------------------------------------

/// Apply sort inputs to a model query.
///
/// Only columns declared as `sortable` participate.
pub fn apply_sorts<M: Model>(
    mut query: ModelQuery<M>,
    sorts: &[DatatableSortInput],
    columns: &[DatatableColumn<M>],
    table_name: &str,
) -> Result<ModelQuery<M>> {
    for sort in sorts {
        let col = match columns.iter().find(|c| c.name == sort.field) {
            Some(c) if c.sortable => c,
            Some(_) => {
                return Err(Error::message(format!(
                    "column '{}' is not sortable",
                    sort.field
                )));
            }
            None => {
                return Err(Error::message(format!(
                    "unknown sort field '{}'",
                    sort.field
                )));
            }
        };

        let col_ref = ColumnRef::new(table_name, &col.name).typed(col.db_type());
        let order_by = OrderBy {
            expr: Expr::column(col_ref),
            direction: sort.direction,
        };
        query = query.order_by(order_by);
    }

    Ok(query)
}

/// Apply default sort declarations (used when request has no sort).
pub fn apply_default_sorts<M: Model>(
    mut query: ModelQuery<M>,
    sorts: &[super::sort::DatatableSort<M>],
    table_name: &str,
) -> Result<ModelQuery<M>> {
    for sort in sorts {
        let col_ref = ColumnRef::new(table_name, &sort.column_name);
        let order_by = OrderBy {
            expr: Expr::column(col_ref),
            direction: sort.direction,
        };
        query = query.order_by(order_by);
    }
    Ok(query)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expect_text(value: &DatatableFilterValue) -> Result<String> {
    match value {
        DatatableFilterValue::Text(s) => Ok(s.clone()),
        _ => Err(Error::message("expected text value for this filter operation")),
    }
}

fn expect_values(value: &DatatableFilterValue) -> Result<Vec<String>> {
    match value {
        DatatableFilterValue::Values(vs) => Ok(vs.clone()),
        DatatableFilterValue::Text(s) => Ok(vec![s.clone()]),
        _ => Err(Error::message(
            "expected list of values for In filter operation",
        )),
    }
}

fn filter_value_to_db(value: &DatatableFilterValue, db_type: DbType) -> Result<DbValue> {
    match value {
        DatatableFilterValue::Text(s) => text_to_db_value(s, db_type),
        DatatableFilterValue::Bool(b) => Ok(DbValue::Bool(*b)),
        DatatableFilterValue::Number(n) => number_to_db_value(*n, db_type),
        DatatableFilterValue::Values(vs) if vs.len() == 1 => {
            text_to_db_value(&vs[0], db_type)
        }
        _ => Err(Error::message(
            "cannot convert this filter value to a database value",
        )),
    }
}

fn text_to_db_value(text: &str, db_type: DbType) -> Result<DbValue> {
    match db_type {
        DbType::Bool => text
            .parse::<bool>()
            .map(DbValue::Bool)
            .map_err(|e| Error::message(format!("invalid boolean '{}': {e}", text))),
        DbType::Int16 => text
            .parse::<i16>()
            .map(DbValue::Int16)
            .map_err(|e| Error::message(format!("invalid integer '{}': {e}", text))),
        DbType::Int32 => text
            .parse::<i32>()
            .map(DbValue::Int32)
            .map_err(|e| Error::message(format!("invalid integer '{}': {e}", text))),
        DbType::Int64 => text
            .parse::<i64>()
            .map(DbValue::Int64)
            .map_err(|e| Error::message(format!("invalid integer '{}': {e}", text))),
        DbType::Float32 => text
            .parse::<f32>()
            .map(DbValue::Float32)
            .map_err(|e| Error::message(format!("invalid float '{}': {e}", text))),
        DbType::Float64 => text
            .parse::<f64>()
            .map(DbValue::Float64)
            .map_err(|e| Error::message(format!("invalid float '{}': {e}", text))),
        DbType::Date => text
            .parse::<Date>()
            .map(DbValue::Date)
            .map_err(|e| Error::message(format!("invalid date '{}': {e}", text))),
        DbType::Timestamp => text
            .parse::<LocalDateTime>()
            .map(DbValue::Timestamp)
            .map_err(|e| Error::message(format!("invalid timestamp '{}': {e}", text))),
        DbType::TimestampTz => text
            .parse::<DateTime>()
            .map(DbValue::TimestampTz)
            .map_err(|e| Error::message(format!("invalid timestamptz '{}': {e}", text))),
        DbType::Uuid => uuid::Uuid::parse_str(text)
            .map(DbValue::Uuid)
            .map_err(|e| Error::message(format!("invalid uuid '{}': {e}", text))),
        _ => Ok(DbValue::Text(text.to_string())),
    }
}

fn number_to_db_value(n: i64, db_type: DbType) -> Result<DbValue> {
    match db_type {
        DbType::Int16 => Ok(DbValue::Int16(n as i16)),
        DbType::Int32 => Ok(DbValue::Int32(n as i32)),
        DbType::Int64 => Ok(DbValue::Int64(n)),
        DbType::Float32 => Ok(DbValue::Float32(n as f32)),
        DbType::Float64 => Ok(DbValue::Float64(n as f64)),
        _ => Ok(DbValue::Int64(n)),
    }
}
