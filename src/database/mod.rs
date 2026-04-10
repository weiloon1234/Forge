mod aggregate;
pub mod ast;
pub mod compiler;
pub(crate) mod lifecycle;
mod model;
mod projection;
mod query;
mod relation;
mod runtime;

pub use aggregate::AggregateProjection;
pub use ast::{
    AggregateExpr, AggregateFn, AggregateNode, BinaryExpr, BinaryOperator, CaseExpr, CaseWhen,
    ColumnRef, ComparisonOp, Condition, CteMaterialization, CteNode, DbType, DbValue, DeleteNode,
    Expr, FromItem, FunctionCall, InsertNode, InsertSource, JoinKind, JoinNode, JsonPathExpr,
    JsonPathMode, JsonPathSegment, JsonPredicateOp, JsonPredicateValue, LockBehavior, LockClause,
    LockStrength, Numeric, OnConflictAction, OnConflictNode, OnConflictTarget, OrderBy,
    OrderDirection, QueryAst, QueryBody, RelationKind, RelationNode, SelectItem, SelectNode,
    SetOperationNode, SetOperator, TableRef, UnaryExpr, UnaryOperator, UpdateNode, WindowExpr,
    WindowFrame, WindowFrameBound, WindowFrameUnits, WindowSpec,
};
pub use compiler::{CompiledSql, PostgresCompiler};
pub use lifecycle::{MigrationContext, MigrationFile, SeederContext, SeederFile};
pub use model::{
    Column, ColumnInfo, CreateDraft, FromDbValue, IntoColumnValue, IntoFieldValue, Loaded, Model,
    ModelCreatedEvent, ModelCreatingEvent, ModelDeletedEvent, ModelDeletingEvent, ModelHookContext,
    ModelInstanceWriteExt, ModelLifecycle, ModelLifecycleSnapshot, ModelUpdatedEvent,
    ModelUpdatingEvent, ModelWriteExecutor, NoModelLifecycle, PersistedModel, TableMeta, ToDbValue,
    UpdateDraft,
};
pub use projection::{Projection, ProjectionField, ProjectionFieldInfo, ProjectionMeta};
pub use query::{
    Case, CreateManyModel, CreateModel, CreateRow, Cte, DeleteModel, JsonExprBuilder, ModelQuery,
    Paginated, Pagination, ProjectionQuery, Query, Sql, UpdateModel, Window, WindowBuilder,
};
pub use relation::{
    belongs_to, has_many, has_one, many_to_many, ManyToManyDef, RelationAggregateDef, RelationDef,
};
pub use runtime::{
    DatabaseManager, DatabaseTransaction, DbRecord, DbRecordStream, QueryExecutionOptions,
    QueryExecutor,
};

pub(crate) use lifecycle::{
    builtin_cli_registrar, MigrationRegistryBuilder, MigrationRegistryHandle,
    SeederRegistryBuilder, SeederRegistryHandle,
};
