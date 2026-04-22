use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::database::{DbRecord, DbType, DbValue, Model, QueryExecutor};
use crate::foundation::{Error, Result};

#[derive(Debug, Serialize, Deserialize, crate::Model)]
#[forge(model = "audit_logs", audit = false)]
pub struct AuditLog {
    pub id: crate::ModelId<AuditLog>,
    pub event_type: String,
    pub subject_model: String,
    pub subject_table: String,
    pub subject_id: String,
    pub area: Option<String>,
    pub actor_guard: Option<String>,
    pub actor_id: Option<String>,
    pub request_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub before_data: Option<serde_json::Value>,
    pub after_data: Option<serde_json::Value>,
    pub changes: Option<serde_json::Value>,
    pub created_at: crate::DateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditEventType {
    Created,
    Updated,
    SoftDeleted,
    Restored,
    Deleted,
}

impl AuditEventType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::SoftDeleted => "soft_deleted",
            Self::Restored => "restored",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AuditPayload {
    before_data: Option<serde_json::Value>,
    after_data: Option<serde_json::Value>,
    changes: Option<serde_json::Value>,
}

pub(crate) struct AuditManager {
    availability: AtomicU8,
    warned_missing: AtomicBool,
}

impl AuditManager {
    pub(crate) fn new() -> Self {
        Self {
            availability: AtomicU8::new(0),
            warned_missing: AtomicBool::new(false),
        }
    }

    pub(crate) fn active_for<M>(&self) -> bool
    where
        M: Model,
    {
        let request = crate::logging::current_request();
        self.active_for_request::<M>(request.as_ref())
    }

    pub(crate) fn active_for_request<M>(
        &self,
        request: Option<&crate::logging::CurrentRequest>,
    ) -> bool
    where
        M: Model,
    {
        M::audit_enabled()
            && M::table_meta().name() != "audit_logs"
            && request
                .and_then(|request| request.audit_area.as_deref())
                .is_some()
    }

    async fn table_available(&self, executor: &dyn QueryExecutor) -> Result<bool> {
        match self.availability.load(Ordering::Relaxed) {
            1 => return Ok(true),
            2 => return Ok(false),
            _ => {}
        }

        let rows = executor
            .raw_query(
                r#"
                SELECT
                    to_regclass('audit_logs')::TEXT AS audit_table,
                    EXISTS (
                        SELECT 1
                        FROM information_schema.columns
                        WHERE table_schema = current_schema()
                          AND table_name = 'audit_logs'
                          AND column_name = 'area'
                    ) AS audit_area_column
                "#,
                &[],
            )
            .await?;
        let table_exists = rows
            .first()
            .and_then(|row| row.get("audit_table"))
            .is_some_and(|value| !matches!(value, DbValue::Null(_)));
        let has_area_column = rows
            .first()
            .and_then(|row| row.get("audit_area_column"))
            .is_some_and(|value| matches!(value, DbValue::Bool(true)));
        let available = table_exists && has_area_column;

        self.availability
            .store(if available { 1 } else { 2 }, Ordering::Relaxed);

        if !available && !self.warned_missing.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: "forge.audit",
                "audit_logs table or `area` column is missing; built-in audit logging is disabled until framework migrations are published and applied"
            );
        }

        Ok(available)
    }
}

pub(crate) async fn write_model_audit<M>(
    context: &crate::database::ModelHookContext<'_>,
    event_type: AuditEventType,
    before: Option<&DbRecord>,
    after: Option<&DbRecord>,
) -> Result<()>
where
    M: Model,
{
    let audit = context.app().audit()?;
    let request = crate::logging::current_request();
    if !audit.active_for_request::<M>(request.as_ref())
        || !audit.table_available(context.transaction()).await?
    {
        return Ok(());
    }

    let payload = build_payload(event_type, before, after, M::audit_excluded_fields());
    let subject_source = after.or(before).ok_or_else(|| {
        Error::message(format!(
            "audit logging for `{}` requires a before or after record",
            M::table_meta().name()
        ))
    })?;
    let actor = context.actor();

    context
        .transaction()
        .raw_execute(
            r#"
            INSERT INTO audit_logs (
                event_type,
                subject_model,
                subject_table,
                subject_id,
                area,
                actor_guard,
                actor_id,
                request_id,
                ip,
                user_agent,
                before_data,
                after_data,
                changes
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
            &[
                DbValue::Text(event_type.as_str().to_string()),
                DbValue::Text(std::any::type_name::<M>().to_string()),
                DbValue::Text(M::table_meta().name().to_string()),
                DbValue::Text(subject_id_for_record::<M>(subject_source)?),
                nullable_text(request.as_ref().and_then(|value| value.audit_area.clone())),
                nullable_text(actor.map(|value| value.guard.as_ref().to_string())),
                nullable_text(actor.map(|value| value.id.clone())),
                nullable_text(request.as_ref().and_then(|value| value.request_id.clone())),
                nullable_text(
                    request
                        .as_ref()
                        .and_then(|value| value.ip.map(|ip| ip.to_string())),
                ),
                nullable_text(request.as_ref().and_then(|value| value.user_agent.clone())),
                nullable_json(payload.before_data),
                nullable_json(payload.after_data),
                nullable_json(payload.changes),
            ],
        )
        .await?;

    Ok(())
}

pub(crate) fn record_with_assignments(
    current: &DbRecord,
    assignments: &[(crate::ColumnRef, crate::Expr)],
) -> DbRecord {
    let mut record = current.clone();
    for (column, expr) in assignments {
        if let crate::Expr::Value(value) = expr {
            record.insert(column.name.clone(), value.clone());
        }
    }
    record
}

fn subject_id_for_record<M>(record: &DbRecord) -> Result<String>
where
    M: Model,
{
    let primary_key = M::table_meta()
        .primary_key_column_info()
        .ok_or_else(|| Error::message("audit subject is missing a primary key column"))?;
    let value = record.get(primary_key.name).ok_or_else(|| {
        Error::message(format!(
            "audit subject record is missing primary key `{}`",
            primary_key.name
        ))
    })?;
    db_value_to_string(value)
}

fn nullable_text(value: Option<String>) -> DbValue {
    match value {
        Some(value) => DbValue::Text(value),
        None => DbValue::Null(DbType::Text),
    }
}

fn nullable_json(value: Option<serde_json::Value>) -> DbValue {
    match value {
        Some(value) => DbValue::Json(value),
        None => DbValue::Null(DbType::Json),
    }
}

fn build_payload(
    event_type: AuditEventType,
    before: Option<&DbRecord>,
    after: Option<&DbRecord>,
    excluded_fields: &[&str],
) -> AuditPayload {
    match event_type {
        AuditEventType::Created => AuditPayload {
            before_data: None,
            after_data: after.map(|record| record_to_json(record, excluded_fields)),
            changes: None,
        },
        AuditEventType::Deleted => AuditPayload {
            before_data: before.map(|record| record_to_json(record, excluded_fields)),
            after_data: None,
            changes: None,
        },
        AuditEventType::Updated | AuditEventType::SoftDeleted | AuditEventType::Restored => {
            let before_data = before.map(|record| record_to_json(record, excluded_fields));
            let after_data = after.map(|record| record_to_json(record, excluded_fields));
            AuditPayload {
                changes: build_changes(before, after, excluded_fields),
                before_data,
                after_data,
            }
        }
    }
}

fn build_changes(
    before: Option<&DbRecord>,
    after: Option<&DbRecord>,
    excluded_fields: &[&str],
) -> Option<serde_json::Value> {
    let (Some(before), Some(after)) = (before, after) else {
        return None;
    };

    let excluded: BTreeSet<&str> = excluded_fields.iter().copied().collect();
    let mut keys = BTreeSet::new();
    for (key, _) in before.iter() {
        if !excluded.contains(key.as_str()) {
            keys.insert(key.clone());
        }
    }
    for (key, _) in after.iter() {
        if !excluded.contains(key.as_str()) {
            keys.insert(key.clone());
        }
    }

    let mut changes = serde_json::Map::new();
    for key in keys {
        let before_value = before.get(&key).map(db_value_to_json).unwrap_or_default();
        let after_value = after.get(&key).map(db_value_to_json).unwrap_or_default();
        if before_value != after_value {
            changes.insert(
                key,
                serde_json::json!({
                    "before": before_value,
                    "after": after_value,
                }),
            );
        }
    }

    if changes.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(changes))
    }
}

fn record_to_json(record: &DbRecord, excluded_fields: &[&str]) -> serde_json::Value {
    let excluded: BTreeSet<&str> = excluded_fields.iter().copied().collect();
    let mut values = serde_json::Map::new();
    for (key, value) in record.iter() {
        if excluded.contains(key.as_str()) {
            continue;
        }
        values.insert(key.clone(), db_value_to_json(value));
    }
    serde_json::Value::Object(values)
}

fn db_value_to_json(value: &DbValue) -> serde_json::Value {
    match value {
        DbValue::Null(_) => serde_json::Value::Null,
        DbValue::Int16(value) => serde_json::json!(value),
        DbValue::Int32(value) => serde_json::json!(value),
        DbValue::Int64(value) => serde_json::json!(value),
        DbValue::Bool(value) => serde_json::json!(value),
        DbValue::Float32(value) => serde_json::json!(value),
        DbValue::Float64(value) => serde_json::json!(value),
        DbValue::Numeric(value) => serde_json::Value::String(value.to_string()),
        DbValue::Text(value) => serde_json::Value::String(value.clone()),
        DbValue::Json(value) => value.clone(),
        DbValue::Uuid(value) => serde_json::Value::String(value.to_string()),
        DbValue::TimestampTz(value) => serde_json::Value::String(value.to_string()),
        DbValue::Timestamp(value) => serde_json::Value::String(value.to_string()),
        DbValue::Date(value) => serde_json::Value::String(value.to_string()),
        DbValue::Time(value) => serde_json::Value::String(value.to_string()),
        DbValue::Bytea(value) => {
            serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(value))
        }
        DbValue::Int16Array(value) => serde_json::json!(value),
        DbValue::Int32Array(value) => serde_json::json!(value),
        DbValue::Int64Array(value) => serde_json::json!(value),
        DbValue::BoolArray(value) => serde_json::json!(value),
        DbValue::Float32Array(value) => serde_json::json!(value),
        DbValue::Float64Array(value) => serde_json::json!(value),
        DbValue::NumericArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::TextArray(value) => serde_json::json!(value),
        DbValue::JsonArray(value) => serde_json::Value::Array(value.clone()),
        DbValue::UuidArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::TimestampTzArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::TimestampArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::DateArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::TimeArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| serde_json::Value::String(entry.to_string()))
                .collect(),
        ),
        DbValue::ByteaArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|entry| {
                    serde_json::Value::String(
                        base64::engine::general_purpose::STANDARD.encode(entry),
                    )
                })
                .collect(),
        ),
    }
}

fn db_value_to_string(value: &DbValue) -> Result<String> {
    Ok(match value {
        DbValue::Null(_) => {
            return Err(Error::message(
                "audit subject primary key cannot be null after persistence",
            ));
        }
        DbValue::Int16(value) => value.to_string(),
        DbValue::Int32(value) => value.to_string(),
        DbValue::Int64(value) => value.to_string(),
        DbValue::Bool(value) => value.to_string(),
        DbValue::Float32(value) => value.to_string(),
        DbValue::Float64(value) => value.to_string(),
        DbValue::Numeric(value) => value.to_string(),
        DbValue::Text(value) => value.clone(),
        DbValue::Json(value) => value.to_string(),
        DbValue::Uuid(value) => value.to_string(),
        DbValue::TimestampTz(value) => value.to_string(),
        DbValue::Timestamp(value) => value.to_string(),
        DbValue::Date(value) => value.to_string(),
        DbValue::Time(value) => value.to_string(),
        DbValue::Bytea(value) => base64::engine::general_purpose::STANDARD.encode(value),
        DbValue::Int16Array(_)
        | DbValue::Int32Array(_)
        | DbValue::Int64Array(_)
        | DbValue::BoolArray(_)
        | DbValue::Float32Array(_)
        | DbValue::Float64Array(_)
        | DbValue::NumericArray(_)
        | DbValue::TextArray(_)
        | DbValue::JsonArray(_)
        | DbValue::UuidArray(_)
        | DbValue::TimestampTzArray(_)
        | DbValue::TimestampArray(_)
        | DbValue::DateArray(_)
        | DbValue::TimeArray(_)
        | DbValue::ByteaArray(_) => {
            return Err(Error::message(
                "audit subject primary key cannot use an array value",
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{build_payload, record_with_assignments, AuditEventType};
    use crate::{ColumnRef, DbRecord, DbType, DbValue, Expr};

    fn record(entries: &[(&str, DbValue)]) -> DbRecord {
        let mut record = DbRecord::new();
        for (key, value) in entries {
            record.insert(*key, value.clone());
        }
        record
    }

    #[test]
    fn created_payload_uses_after_data_only() {
        let after = record(&[
            ("id", DbValue::Int64(1)),
            ("title", DbValue::Text("Hello".into())),
        ]);

        let payload = build_payload(AuditEventType::Created, None, Some(&after), &[]);

        assert!(payload.before_data.is_none());
        assert_eq!(payload.after_data.unwrap()["title"], "Hello");
        assert!(payload.changes.is_none());
    }

    #[test]
    fn updated_payload_tracks_dirty_fields_only() {
        let before = record(&[
            ("id", DbValue::Int64(1)),
            ("title", DbValue::Text("Before".into())),
            ("updated_at", DbValue::Text("old".into())),
        ]);
        let after = record(&[
            ("id", DbValue::Int64(1)),
            ("title", DbValue::Text("After".into())),
            ("updated_at", DbValue::Text("new".into())),
        ]);

        let payload = build_payload(
            AuditEventType::Updated,
            Some(&before),
            Some(&after),
            &["updated_at"],
        );

        let changes = payload.changes.unwrap();
        assert_eq!(changes["title"]["before"], "Before");
        assert_eq!(changes["title"]["after"], "After");
        assert!(changes.get("updated_at").is_none());
    }

    #[test]
    fn deleted_payload_uses_before_data_only() {
        let before = record(&[
            ("id", DbValue::Int64(1)),
            ("title", DbValue::Text("Gone".into())),
        ]);

        let payload = build_payload(AuditEventType::Deleted, Some(&before), None, &[]);

        assert_eq!(payload.before_data.unwrap()["title"], "Gone");
        assert!(payload.after_data.is_none());
        assert!(payload.changes.is_none());
    }

    #[test]
    fn soft_delete_payload_marks_deleted_at_change() {
        let before = record(&[
            ("id", DbValue::Int64(1)),
            ("deleted_at", DbValue::Null(DbType::TimestampTz)),
        ]);
        let after = record_with_assignments(
            &before,
            &[(
                ColumnRef::new("posts", "deleted_at").typed(DbType::TimestampTz),
                Expr::value(DbValue::Text("2026-04-22T12:00:00Z".into())),
            )],
        );

        let payload = build_payload(
            AuditEventType::SoftDeleted,
            Some(&before),
            Some(&after),
            &[],
        );

        let changes = payload.changes.unwrap();
        assert!(changes.get("deleted_at").is_some());
    }

    #[test]
    fn restored_payload_marks_deleted_at_change() {
        let before = record(&[
            ("id", DbValue::Int64(1)),
            ("deleted_at", DbValue::Text("2026-04-22T12:00:00Z".into())),
        ]);
        let after = record(&[
            ("id", DbValue::Int64(1)),
            ("deleted_at", DbValue::Null(DbType::TimestampTz)),
        ]);

        let payload = build_payload(AuditEventType::Restored, Some(&before), Some(&after), &[]);

        let changes = payload.changes.unwrap();
        assert!(changes.get("deleted_at").is_some());
    }
}
