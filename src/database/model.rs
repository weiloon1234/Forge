use std::{
    any::type_name,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

use crate::config::DatabaseModelConfig;
use crate::events::{Event, EventBus, EventOrigin};
use crate::foundation::{AppContext, Error, Result};
use crate::support::{Date, DateTime, EventId, LocalDateTime, ModelId, Time};

use super::ast::{
    ColumnRef, ComparisonOp, Condition, DbType, DbValue, Expr, Numeric, OrderBy, SelectItem,
    TableRef,
};
use super::query::{
    CreateManyModel, CreateModel, DeleteModel, ModelQuery, RestoreModel, UpdateModel,
};
use super::runtime::{DatabaseManager, DatabaseTransaction, DbRecord, QueryExecutor};

pub trait ToDbValue {
    fn to_db_value(self) -> DbValue;
}

pub trait FromDbValue: Sized {
    fn from_db_value(value: &DbValue) -> Result<Self>;
}

pub trait IntoColumnValue<T> {
    fn into_column_value(self) -> T;
}

pub trait IntoFieldValue<T> {
    fn into_field_value(self, db_type: DbType) -> DbValue;
}

impl<T> IntoColumnValue<T> for T {
    fn into_column_value(self) -> T {
        self
    }
}

impl<T> IntoColumnValue<Option<T>> for T {
    fn into_column_value(self) -> Option<T> {
        Some(self)
    }
}

impl IntoColumnValue<String> for &str {
    fn into_column_value(self) -> String {
        self.to_string()
    }
}

impl IntoColumnValue<Option<String>> for &str {
    fn into_column_value(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl<T, V> IntoFieldValue<T> for V
where
    V: IntoColumnValue<T>,
    T: ToDbValue,
{
    fn into_field_value(self, _db_type: DbType) -> DbValue {
        self.into_column_value().to_db_value()
    }
}

impl<T, V> IntoFieldValue<Option<T>> for V
where
    V: IntoColumnValue<Option<T>>,
    T: ToDbValue,
{
    fn into_field_value(self, db_type: DbType) -> DbValue {
        match self.into_column_value() {
            Some(value) => value.to_db_value(),
            None => DbValue::Null(db_type),
        }
    }
}

impl ToDbValue for DbValue {
    fn to_db_value(self) -> DbValue {
        self
    }
}

impl ToDbValue for i64 {
    fn to_db_value(self) -> DbValue {
        DbValue::Int64(self)
    }
}

impl ToDbValue for i16 {
    fn to_db_value(self) -> DbValue {
        DbValue::Int16(self)
    }
}

impl ToDbValue for i32 {
    fn to_db_value(self) -> DbValue {
        DbValue::Int32(self)
    }
}

impl ToDbValue for bool {
    fn to_db_value(self) -> DbValue {
        DbValue::Bool(self)
    }
}

impl ToDbValue for f64 {
    fn to_db_value(self) -> DbValue {
        DbValue::Float64(self)
    }
}

impl ToDbValue for f32 {
    fn to_db_value(self) -> DbValue {
        DbValue::Float32(self)
    }
}

impl ToDbValue for Numeric {
    fn to_db_value(self) -> DbValue {
        DbValue::Numeric(self)
    }
}

impl ToDbValue for String {
    fn to_db_value(self) -> DbValue {
        DbValue::Text(self)
    }
}

impl ToDbValue for &str {
    fn to_db_value(self) -> DbValue {
        DbValue::Text(self.to_string())
    }
}

impl ToDbValue for serde_json::Value {
    fn to_db_value(self) -> DbValue {
        DbValue::Json(self)
    }
}

impl ToDbValue for Uuid {
    fn to_db_value(self) -> DbValue {
        DbValue::Uuid(self)
    }
}

impl<M> ToDbValue for ModelId<M> {
    fn to_db_value(self) -> DbValue {
        DbValue::Uuid(self.into_uuid())
    }
}

impl ToDbValue for DateTime {
    fn to_db_value(self) -> DbValue {
        DbValue::TimestampTz(self)
    }
}

impl ToDbValue for LocalDateTime {
    fn to_db_value(self) -> DbValue {
        DbValue::Timestamp(self)
    }
}

impl ToDbValue for Date {
    fn to_db_value(self) -> DbValue {
        DbValue::Date(self)
    }
}

impl ToDbValue for Time {
    fn to_db_value(self) -> DbValue {
        DbValue::Time(self)
    }
}

impl ToDbValue for Vec<u8> {
    fn to_db_value(self) -> DbValue {
        DbValue::Bytea(self)
    }
}

impl FromDbValue for DbValue {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        Ok(value.clone())
    }
}

impl FromDbValue for i64 {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Int64(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected int64, found null")),
            _ => Err(Error::message("expected int64 value")),
        }
    }
}

impl FromDbValue for i16 {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Int16(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected int16, found null")),
            _ => Err(Error::message("expected int16 value")),
        }
    }
}

impl FromDbValue for i32 {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Int32(value) => Ok(*value),
            DbValue::Int16(value) => Ok(i32::from(*value)),
            DbValue::Null(_) => Err(Error::message("expected int32, found null")),
            _ => Err(Error::message("expected int32 value")),
        }
    }
}

impl FromDbValue for bool {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Bool(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected bool, found null")),
            _ => Err(Error::message("expected bool value")),
        }
    }
}

impl FromDbValue for f64 {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Float64(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected float64, found null")),
            _ => Err(Error::message("expected float64 value")),
        }
    }
}

impl FromDbValue for f32 {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Float32(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected float32, found null")),
            _ => Err(Error::message("expected float32 value")),
        }
    }
}

impl FromDbValue for Numeric {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Numeric(value) => Ok(value.clone()),
            DbValue::Text(value) => Numeric::new(value.clone()),
            DbValue::Null(_) => Err(Error::message("expected numeric, found null")),
            _ => Err(Error::message("expected numeric value")),
        }
    }
}

impl FromDbValue for String {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Text(value) => Ok(value.clone()),
            DbValue::Null(_) => Err(Error::message("expected text, found null")),
            _ => Err(Error::message("expected text value")),
        }
    }
}

impl FromDbValue for serde_json::Value {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Json(value) => Ok(value.clone()),
            DbValue::Null(_) => Err(Error::message("expected json, found null")),
            _ => Err(Error::message("expected json value")),
        }
    }
}

impl FromDbValue for Uuid {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Uuid(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected uuid, found null")),
            _ => Err(Error::message("expected uuid value")),
        }
    }
}

impl<M> FromDbValue for ModelId<M> {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        Uuid::from_db_value(value).map(ModelId::from_uuid)
    }
}

impl FromDbValue for DateTime {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::TimestampTz(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected timestamptz, found null")),
            _ => Err(Error::message("expected timestamptz value")),
        }
    }
}

impl FromDbValue for LocalDateTime {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Timestamp(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected timestamp, found null")),
            _ => Err(Error::message("expected timestamp value")),
        }
    }
}

impl FromDbValue for Date {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Date(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected date, found null")),
            _ => Err(Error::message("expected date value")),
        }
    }
}

impl FromDbValue for Time {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Time(value) => Ok(*value),
            DbValue::Null(_) => Err(Error::message("expected time, found null")),
            _ => Err(Error::message("expected time value")),
        }
    }
}

impl FromDbValue for Vec<u8> {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Bytea(value) => Ok(value.clone()),
            DbValue::Null(_) => Err(Error::message("expected bytea, found null")),
            _ => Err(Error::message("expected bytea value")),
        }
    }
}

impl<T> FromDbValue for Option<T>
where
    T: FromDbValue,
{
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Null(_) => Ok(None),
            _ => T::from_db_value(value).map(Some),
        }
    }
}

impl DbRecord {
    pub fn decode<T>(&self, key: &str) -> Result<T>
    where
        T: FromDbValue,
    {
        let value = self
            .get(key)
            .ok_or_else(|| Error::message(format!("missing column `{key}` in record")))?;
        T::from_db_value(value)
    }

    pub fn decode_column<M, T>(&self, column: Column<M, T>) -> Result<T>
    where
        T: FromDbValue,
    {
        self.decode(column.name())
    }
}

impl IntoColumnValue<Numeric> for i64 {
    fn into_column_value(self) -> Numeric {
        Numeric::from(self)
    }
}

impl IntoColumnValue<Numeric> for i32 {
    fn into_column_value(self) -> Numeric {
        Numeric::from(i64::from(self))
    }
}

trait DbArrayElement: Sized {
    fn to_array_value(values: Vec<Self>) -> DbValue;
    fn from_array_value(value: &DbValue) -> Result<Vec<Self>>;
}

macro_rules! impl_array_value {
    ($ty:ty, $variant:ident, $name:literal) => {
        impl DbArrayElement for $ty {
            fn to_array_value(values: Vec<Self>) -> DbValue {
                DbValue::$variant(values)
            }

            fn from_array_value(value: &DbValue) -> Result<Vec<Self>> {
                match value {
                    DbValue::$variant(values) => Ok(values.clone()),
                    DbValue::Null(_) => Err(Error::message(concat!(
                        "expected ",
                        $name,
                        " array, found null"
                    ))),
                    _ => Err(Error::message(concat!("expected ", $name, " array value"))),
                }
            }
        }
    };
}

impl_array_value!(i16, Int16Array, "int16");
impl_array_value!(i32, Int32Array, "int32");
impl_array_value!(i64, Int64Array, "int64");
impl_array_value!(bool, BoolArray, "bool");
impl_array_value!(f32, Float32Array, "float32");
impl_array_value!(f64, Float64Array, "float64");
impl_array_value!(Numeric, NumericArray, "numeric");
impl_array_value!(String, TextArray, "text");
impl_array_value!(serde_json::Value, JsonArray, "json");
impl_array_value!(Uuid, UuidArray, "uuid");
impl_array_value!(DateTime, TimestampTzArray, "timestamptz");
impl_array_value!(LocalDateTime, TimestampArray, "timestamp");
impl_array_value!(Date, DateArray, "date");
impl_array_value!(Time, TimeArray, "time");
impl_array_value!(Vec<u8>, ByteaArray, "bytea");

impl<T> ToDbValue for Vec<T>
where
    T: DbArrayElement,
{
    fn to_db_value(self) -> DbValue {
        T::to_array_value(self)
    }
}

impl<M> DbArrayElement for ModelId<M> {
    fn to_array_value(values: Vec<Self>) -> DbValue {
        DbValue::UuidArray(values.into_iter().map(ModelId::into_uuid).collect())
    }

    fn from_array_value(value: &DbValue) -> Result<Vec<Self>> {
        match value {
            DbValue::UuidArray(values) => {
                Ok(values.iter().copied().map(ModelId::from_uuid).collect())
            }
            DbValue::Null(_) => Err(Error::message("expected uuid array, found null")),
            _ => Err(Error::message("expected uuid array value")),
        }
    }
}

impl<T> FromDbValue for Vec<T>
where
    T: DbArrayElement,
{
    fn from_db_value(value: &DbValue) -> Result<Self> {
        T::from_array_value(value)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ColumnInfo {
    pub name: &'static str,
    pub db_type: DbType,
    write_mutator: Option<ModelFieldWriteMutator>,
}

#[doc(hidden)]
pub type ModelFieldWriteMutatorFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DbValue>> + Send + 'a>>;

#[doc(hidden)]
pub type ModelFieldWriteMutator =
    for<'a> fn(&'a ModelHookContext<'a>, DbValue) -> ModelFieldWriteMutatorFuture<'a>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFeatureSetting {
    Default,
    Enabled,
    Disabled,
}

impl ModelFeatureSetting {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelBehavior {
    pub timestamps: ModelFeatureSetting,
    pub soft_deletes: ModelFeatureSetting,
}

impl ModelBehavior {
    pub const fn new(timestamps: ModelFeatureSetting, soft_deletes: ModelFeatureSetting) -> Self {
        Self {
            timestamps,
            soft_deletes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelPrimaryKeyStrategy {
    UuidV7,
    Manual,
}

impl ModelPrimaryKeyStrategy {
    pub const fn generates_value(self) -> bool {
        matches!(self, Self::UuidV7)
    }
}

fn runtime_model_defaults_lock() -> &'static Mutex<DatabaseModelConfig> {
    static DEFAULTS: OnceLock<Mutex<DatabaseModelConfig>> = OnceLock::new();
    DEFAULTS.get_or_init(|| Mutex::new(DatabaseModelConfig::default()))
}

fn runtime_model_defaults() -> DatabaseModelConfig {
    runtime_model_defaults_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub(crate) fn set_runtime_model_defaults(defaults: DatabaseModelConfig) {
    *runtime_model_defaults_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = defaults;
}

impl ColumnInfo {
    pub const fn new(name: &'static str, db_type: DbType) -> Self {
        Self {
            name,
            db_type,
            write_mutator: None,
        }
    }

    pub const fn with_write_mutator(mut self, write_mutator: ModelFieldWriteMutator) -> Self {
        self.write_mutator = Some(write_mutator);
        self
    }

    pub const fn write_mutator(&self) -> Option<ModelFieldWriteMutator> {
        self.write_mutator
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column<M, T> {
    table: &'static str,
    name: &'static str,
    db_type: DbType,
    _marker: PhantomData<fn() -> (M, T)>,
}

impl<M, T> Column<M, T> {
    pub const fn new(table: &'static str, name: &'static str, db_type: DbType) -> Self {
        Self {
            table,
            name,
            db_type,
            _marker: PhantomData,
        }
    }

    pub const fn info(&self) -> ColumnInfo {
        ColumnInfo::new(self.name, self.db_type)
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn db_type(&self) -> DbType {
        self.db_type
    }

    pub fn column_ref(&self) -> ColumnRef {
        ColumnRef::new(self.table, self.name).typed(self.db_type)
    }

    pub fn asc(&self) -> OrderBy {
        OrderBy::asc(self.column_ref())
    }

    pub fn desc(&self) -> OrderBy {
        OrderBy::desc(self.column_ref())
    }

    pub fn eq<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Eq,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn not_eq<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::NotEq,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn gt<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Gt,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn gte<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Gte,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn lt<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Lt,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn lte<V>(&self, value: V) -> Condition
    where
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Lte,
            Expr::value(value.into_column_value().to_db_value()),
        )
    }

    pub fn in_list<I, V>(&self, values: I) -> Condition
    where
        I: IntoIterator<Item = V>,
        V: IntoColumnValue<T>,
        T: ToDbValue,
    {
        Condition::InList {
            expr: Expr::column(self.column_ref()),
            values: values
                .into_iter()
                .map(|value| value.into_column_value().to_db_value())
                .collect(),
        }
    }

    pub fn is_null(&self) -> Condition {
        Condition::IsNull(self.column_ref())
    }

    pub fn is_not_null(&self) -> Condition {
        Condition::IsNotNull(self.column_ref())
    }

    pub fn like(&self, value: impl Into<String>) -> Condition {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::Like,
            Expr::value(DbValue::Text(value.into())),
        )
    }

    pub fn ieq(&self, value: impl Into<String>) -> Condition {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::IEq,
            Expr::value(DbValue::Text(value.into())),
        )
    }

    pub fn not_like(&self, value: impl Into<String>) -> Condition {
        Condition::compare(
            Expr::column(self.column_ref()),
            ComparisonOp::NotLike,
            Expr::value(DbValue::Text(value.into())),
        )
    }
}

impl<M> Column<M, serde_json::Value> {
    pub fn json(&self) -> super::query::JsonExprBuilder {
        super::query::JsonExprBuilder::new(Expr::column(self.column_ref()))
    }
}

impl<M, T> From<Column<M, T>> for ColumnRef {
    fn from(value: Column<M, T>) -> Self {
        value.column_ref()
    }
}

#[derive(Clone)]
pub struct TableMeta<M> {
    name: &'static str,
    columns: &'static [ColumnInfo],
    primary_key: &'static str,
    primary_key_strategy: ModelPrimaryKeyStrategy,
    behavior: ModelBehavior,
    hydrate: fn(&DbRecord) -> Result<M>,
}

impl<M> TableMeta<M> {
    pub const fn new(
        name: &'static str,
        columns: &'static [ColumnInfo],
        primary_key: &'static str,
        primary_key_strategy: ModelPrimaryKeyStrategy,
        behavior: ModelBehavior,
        hydrate: fn(&DbRecord) -> Result<M>,
    ) -> Self {
        Self {
            name,
            columns,
            primary_key,
            primary_key_strategy,
            behavior,
            hydrate,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub fn table_ref(&self) -> TableRef {
        TableRef::new(self.name)
    }

    pub fn primary_key_ref(&self) -> ColumnRef {
        ColumnRef::new(self.name, self.primary_key)
    }

    pub const fn primary_key_name(&self) -> &'static str {
        self.primary_key
    }

    pub const fn primary_key_strategy(&self) -> ModelPrimaryKeyStrategy {
        self.primary_key_strategy
    }

    pub const fn columns(&self) -> &'static [ColumnInfo] {
        self.columns
    }

    pub const fn behavior(&self) -> ModelBehavior {
        self.behavior
    }

    pub fn column_info(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn primary_key_column_info(&self) -> Option<&ColumnInfo> {
        self.column_info(self.primary_key)
    }

    pub fn created_at_column_info(&self) -> Option<&ColumnInfo> {
        self.column_info("created_at")
    }

    pub fn updated_at_column_info(&self) -> Option<&ColumnInfo> {
        self.column_info("updated_at")
    }

    pub fn deleted_at_column_info(&self) -> Option<&ColumnInfo> {
        self.column_info("deleted_at")
    }

    pub fn timestamps_enabled(&self, _app: &AppContext) -> Result<bool> {
        Ok(match self.behavior.timestamps {
            ModelFeatureSetting::Enabled => true,
            ModelFeatureSetting::Disabled => false,
            ModelFeatureSetting::Default => {
                runtime_model_defaults().timestamps_default
                    && self.created_at_column_info().is_some()
                    && self.updated_at_column_info().is_some()
            }
        })
    }

    pub fn soft_deletes_enabled(&self) -> bool {
        match self.behavior.soft_deletes {
            ModelFeatureSetting::Enabled => true,
            ModelFeatureSetting::Disabled => false,
            ModelFeatureSetting::Default => {
                runtime_model_defaults().soft_deletes_default
                    && self.deleted_at_column_info().is_some()
            }
        }
    }

    pub fn all_select_items(&self) -> Vec<SelectItem> {
        self.columns
            .iter()
            .map(|column| {
                SelectItem::new(
                    ColumnRef::new(self.name, column.name)
                        .typed(column.db_type)
                        .aliased(column.name),
                )
            })
            .collect()
    }

    pub fn hydrate_record(&self, record: &DbRecord) -> Result<M> {
        (self.hydrate)(record)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelLifecycleSnapshot {
    pub model: String,
    pub table: String,
    pub primary_key_column: String,
    pub before: Option<DbRecord>,
    pub after: Option<DbRecord>,
    pub pending: Option<DbRecord>,
}

impl ModelLifecycleSnapshot {
    pub fn for_model<M: Model>(
        before: Option<DbRecord>,
        after: Option<DbRecord>,
        pending: Option<DbRecord>,
    ) -> Self {
        Self {
            model: type_name::<M>().to_string(),
            table: M::table_meta().name().to_string(),
            primary_key_column: M::table_meta().primary_key_name().to_string(),
            before,
            after,
            pending,
        }
    }
}

macro_rules! define_model_event {
    ($name:ident, $id:literal) => {
        #[derive(Clone, Debug, PartialEq, Serialize)]
        pub struct $name {
            pub snapshot: ModelLifecycleSnapshot,
        }

        impl Event for $name {
            const ID: EventId = EventId::new($id);
        }
    };
}

define_model_event!(ModelCreatingEvent, "model.creating");
define_model_event!(ModelCreatedEvent, "model.created");
define_model_event!(ModelUpdatingEvent, "model.updating");
define_model_event!(ModelUpdatedEvent, "model.updated");
define_model_event!(ModelDeletingEvent, "model.deleting");
define_model_event!(ModelDeletedEvent, "model.deleted");

pub struct ModelHookContext<'a> {
    app: &'a AppContext,
    database: Arc<DatabaseManager>,
    transaction: &'a DatabaseTransaction,
    origin: Option<EventOrigin>,
}

impl<'a> ModelHookContext<'a> {
    pub(crate) fn new(
        app: &'a AppContext,
        database: Arc<DatabaseManager>,
        transaction: &'a DatabaseTransaction,
        origin: Option<EventOrigin>,
    ) -> Self {
        Self {
            app,
            database,
            transaction,
            origin,
        }
    }

    pub fn app(&self) -> &AppContext {
        self.app
    }

    pub fn database(&self) -> &DatabaseManager {
        self.database.as_ref()
    }

    pub fn transaction(&self) -> &DatabaseTransaction {
        self.transaction
    }

    pub fn actor(&self) -> Option<&crate::auth::Actor> {
        self.origin
            .as_ref()
            .and_then(|origin| origin.actor.as_ref())
    }

    pub fn origin(&self) -> Option<&EventOrigin> {
        self.origin.as_ref()
    }

    pub fn executor(&self) -> &dyn QueryExecutor {
        self.transaction
    }

    pub fn events(&self) -> Result<Arc<EventBus>> {
        self.app.events()
    }

    pub async fn dispatch<E>(&self, event: E) -> Result<()>
    where
        E: Event,
    {
        self.events()?
            .dispatch_with_origin(event, self.origin.clone())
            .await
    }
}

#[derive(Clone)]
pub struct CreateDraft<M> {
    values: Vec<(ColumnRef, Expr)>,
    _marker: PhantomData<fn() -> M>,
}

impl<M> CreateDraft<M> {
    pub(crate) fn new(values: Vec<(ColumnRef, Expr)>) -> Self {
        Self {
            values,
            _marker: PhantomData,
        }
    }

    pub fn set<T, V>(&mut self, column: Column<M, T>, value: V) -> &mut Self
    where
        V: IntoFieldValue<T>,
    {
        upsert_assignment(
            &mut self.values,
            column.column_ref(),
            Expr::value(value.into_field_value(column.db_type())),
        );
        self
    }

    pub fn set_expr<T>(&mut self, column: Column<M, T>, expr: impl Into<Expr>) -> &mut Self {
        upsert_assignment(&mut self.values, column.column_ref(), expr.into());
        self
    }

    pub fn set_null<T>(&mut self, column: Column<M, T>) -> &mut Self {
        upsert_assignment(
            &mut self.values,
            column.column_ref(),
            Expr::value(DbValue::Null(column.db_type())),
        );
        self
    }

    pub fn assigned_columns(&self) -> Vec<&str> {
        self.values
            .iter()
            .map(|(column, _)| column.name.as_str())
            .collect()
    }

    pub fn pending_record(&self) -> DbRecord {
        pending_record_from_assignments(&self.values)
    }

    pub(crate) fn into_values(self) -> Vec<(ColumnRef, Expr)> {
        self.values
    }
}

#[derive(Clone)]
pub struct UpdateDraft<M> {
    values: Vec<(ColumnRef, Expr)>,
    _marker: PhantomData<fn() -> M>,
}

impl<M> UpdateDraft<M> {
    pub(crate) fn new(values: Vec<(ColumnRef, Expr)>) -> Self {
        Self {
            values,
            _marker: PhantomData,
        }
    }

    pub fn set<T, V>(&mut self, column: Column<M, T>, value: V) -> &mut Self
    where
        V: IntoFieldValue<T>,
    {
        upsert_assignment(
            &mut self.values,
            column.column_ref(),
            Expr::value(value.into_field_value(column.db_type())),
        );
        self
    }

    pub fn set_expr<T>(&mut self, column: Column<M, T>, expr: impl Into<Expr>) -> &mut Self {
        upsert_assignment(&mut self.values, column.column_ref(), expr.into());
        self
    }

    pub fn set_null<T>(&mut self, column: Column<M, T>) -> &mut Self {
        upsert_assignment(
            &mut self.values,
            column.column_ref(),
            Expr::value(DbValue::Null(column.db_type())),
        );
        self
    }

    pub fn changed_columns(&self) -> Vec<&str> {
        self.values
            .iter()
            .map(|(column, _)| column.name.as_str())
            .collect()
    }

    pub fn pending_record(&self) -> DbRecord {
        pending_record_from_assignments(&self.values)
    }

    pub(crate) fn into_values(self) -> Vec<(ColumnRef, Expr)> {
        self.values
    }
}

#[async_trait]
pub trait ModelLifecycle<M>: Send + Sync + 'static
where
    M: Model,
{
    async fn creating(_context: &ModelHookContext<'_>, _draft: &mut CreateDraft<M>) -> Result<()> {
        Ok(())
    }

    async fn created(
        _context: &ModelHookContext<'_>,
        _created: &M,
        _record: &DbRecord,
    ) -> Result<()> {
        Ok(())
    }

    async fn updating(
        _context: &ModelHookContext<'_>,
        _current: &M,
        _draft: &mut UpdateDraft<M>,
    ) -> Result<()> {
        Ok(())
    }

    async fn updated(
        _context: &ModelHookContext<'_>,
        _before: &M,
        _after: &M,
        _before_record: &DbRecord,
        _after_record: &DbRecord,
    ) -> Result<()> {
        Ok(())
    }

    async fn deleting(
        _context: &ModelHookContext<'_>,
        _current: &M,
        _record: &DbRecord,
    ) -> Result<()> {
        Ok(())
    }

    async fn deleted(
        _context: &ModelHookContext<'_>,
        _deleted: &M,
        _record: &DbRecord,
    ) -> Result<()> {
        Ok(())
    }
}

pub struct NoModelLifecycle;

#[async_trait]
impl<M> ModelLifecycle<M> for NoModelLifecycle where M: Model {}

pub trait ModelWriteExecutor: QueryExecutor + Send + Sync {
    fn app_context(&self) -> &AppContext;

    fn active_transaction(&self) -> Option<&DatabaseTransaction> {
        None
    }

    fn actor(&self) -> Option<&crate::auth::Actor> {
        None
    }
}

pub trait Model: Clone + Send + Sync + Sized + 'static {
    type Lifecycle: ModelLifecycle<Self>;

    fn table_meta() -> &'static TableMeta<Self>;

    fn audit_enabled() -> bool {
        true
    }

    fn audit_excluded_fields() -> &'static [&'static str] {
        &[]
    }

    #[doc(hidden)]
    fn model_query() -> ModelQuery<Self> {
        ModelQuery::new(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_create() -> CreateModel<Self> {
        CreateModel::new(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_create_many() -> CreateManyModel<Self> {
        CreateManyModel::new(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_update() -> UpdateModel<Self> {
        UpdateModel::new(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_delete() -> DeleteModel<Self> {
        DeleteModel::new(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_force_delete() -> DeleteModel<Self> {
        DeleteModel::new_force(Self::table_meta())
    }

    #[doc(hidden)]
    fn model_restore() -> RestoreModel<Self> {
        UpdateModel::new_restore(Self::table_meta())
    }
}

pub trait PersistedModel: Model {
    fn persisted_condition(&self) -> Condition;
}

pub trait ModelInstanceWriteExt: PersistedModel {
    fn update(&self) -> UpdateModel<Self> {
        <Self as Model>::model_update().where_(self.persisted_condition())
    }

    fn delete(&self) -> DeleteModel<Self> {
        <Self as Model>::model_delete().where_(self.persisted_condition())
    }

    fn force_delete(&self) -> DeleteModel<Self> {
        <Self as Model>::model_force_delete().where_(self.persisted_condition())
    }

    fn restore(&self) -> RestoreModel<Self> {
        <Self as Model>::model_restore().where_(self.persisted_condition())
    }
}

impl<T> ModelInstanceWriteExt for T where T: PersistedModel {}

pub(crate) fn upsert_assignment(
    values: &mut Vec<(ColumnRef, Expr)>,
    column: ColumnRef,
    expr: Expr,
) {
    if let Some((_, existing)) = values
        .iter_mut()
        .find(|(existing_column, _)| existing_column.name == column.name)
    {
        *existing = expr;
    } else {
        values.push((column, expr));
    }
}

fn pending_record_from_assignments(values: &[(ColumnRef, Expr)]) -> DbRecord {
    let mut record = DbRecord::new();
    for (column, expr) in values {
        if let Expr::Value(value) = expr {
            record.insert(column.name.clone(), value.clone());
        }
    }
    record
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Loaded<T> {
    #[default]
    Unloaded,
    Loaded(T),
}

impl<T> Loaded<T> {
    pub fn new(value: T) -> Self {
        Self::Loaded(value)
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, Self::Loaded(_))
    }

    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::Unloaded => None,
        }
    }

    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::Unloaded => None,
        }
    }
}
