extern crate self as forge;

#[doc(hidden)]
pub mod __private {
    use std::path::PathBuf;

    use crate::database::lifecycle::GeneratedDatabasePaths;
    use crate::database::{MigrationFile, SeederFile};
    use crate::foundation::{Result, ServiceRegistrar};
    use crate::support::{MigrationId, SeederId};

    pub fn register_generated_database_paths(
        registrar: &ServiceRegistrar,
        migration_dirs: Vec<PathBuf>,
        seeder_dirs: Vec<PathBuf>,
    ) -> Result<()> {
        registrar.singleton(GeneratedDatabasePaths::new(migration_dirs, seeder_dirs))
    }

    pub fn register_generated_migration_file<M>(
        registrar: &ServiceRegistrar,
        id: MigrationId,
    ) -> Result<()>
    where
        M: MigrationFile,
    {
        registrar.register_generated_migration_file::<M>(id)
    }

    pub fn register_generated_seeder_file<S>(
        registrar: &ServiceRegistrar,
        id: SeederId,
    ) -> Result<()>
    where
        S: SeederFile,
    {
        registrar.register_generated_seeder_file::<S>(id)
    }
}

#[macro_export]
macro_rules! register_generated_database {
    ($registrar:expr) => {{
        mod __forge_generated_database {
            include!(concat!(env!("OUT_DIR"), "/forge_database_generated.rs"));
        }

        __forge_generated_database::register($registrar)
    }};
}

pub mod auth;
pub mod cli;
pub mod config;
pub mod database;
pub mod events;
pub mod foundation;
pub mod http;
pub mod jobs;
pub mod kernel;
pub mod logging;
pub mod plugin;
pub mod prelude;
pub mod scheduler;
pub mod support;
pub mod validation;
pub mod websocket;

pub use forge_macros::{Model, Projection};

pub use auth::{
    AccessScope, Actor, AuthError, AuthManager, Authorizer, BearerAuthenticator, CurrentActor,
    GuardedAccess, OptionalActor, Policy, StaticBearerAuthenticator,
};
pub use database::{
    belongs_to, has_many, has_one, many_to_many, AggregateExpr, AggregateFn, AggregateNode,
    AggregateProjection, BinaryExpr, BinaryOperator, Case, Column, ColumnInfo, ColumnRef,
    ComparisonOp, Condition, CreateDraft, CreateManyModel, CreateModel, CreateRow, Cte,
    DatabaseManager, DatabaseTransaction, DbRecord, DbRecordStream, DbType, DbValue, DeleteModel,
    Expr, FromDbValue, FromItem, FunctionCall, InsertSource, IntoColumnValue, IntoFieldValue,
    JoinKind, JoinNode, JsonExprBuilder, Loaded, LockBehavior, LockClause, LockStrength,
    ManyToManyDef, MigrationContext, MigrationFile, Model, ModelCreatedEvent, ModelCreatingEvent,
    ModelDeletedEvent, ModelDeletingEvent, ModelHookContext, ModelInstanceWriteExt, ModelLifecycle,
    ModelLifecycleSnapshot, ModelQuery, ModelUpdatedEvent, ModelUpdatingEvent, ModelWriteExecutor,
    NoModelLifecycle, Numeric, OnConflictAction, OnConflictNode, OnConflictTarget, OrderBy,
    OrderDirection, Paginated, Pagination, PersistedModel, Projection, ProjectionField,
    ProjectionFieldInfo, ProjectionMeta, ProjectionQuery, Query, QueryAst, QueryBody,
    QueryExecutionOptions, QueryExecutor, RelationAggregateDef, RelationDef, RelationKind,
    RelationNode, SeederContext, SeederFile, SelectItem, SelectNode, SetOperator, Sql, TableMeta,
    TableRef, ToDbValue, UnaryExpr, UnaryOperator, UpdateDraft, UpdateModel, Window, WindowBuilder,
    WindowExpr, WindowFrame, WindowFrameBound, WindowFrameUnits, WindowSpec,
};
pub use foundation::{
    App, AppBuilder, AppContext, AppTransaction, Container, Error, Result, ServiceProvider,
    ServiceRegistrar,
};
pub use jobs::spawn_worker;
pub use kernel::worker::WorkerKernel;
pub use logging::{
    AuthOutcome, HttpOutcomeClass, JobOutcome, LivenessReport, LogLevel, ObservabilityOptions,
    ProbeResult, ProbeState, ReadinessCheck, ReadinessReport, RequestId, RuntimeBackendKind,
    RuntimeDiagnostics, RuntimeSnapshot, SchedulerLeadershipState, WebSocketConnectionState,
};
pub use plugin::{
    Plugin, PluginAsset, PluginAssetKind, PluginDependency, PluginInstallOptions, PluginManifest,
    PluginRegistrar, PluginRegistry, PluginScaffold, PluginScaffoldOptions, PluginScaffoldVar,
};
pub use scheduler::CronExpression;
pub use support::{
    ChannelEventId, ChannelId, CommandId, EventId, GuardId, JobId, MigrationId, PermissionId,
    PluginAssetId, PluginId, PluginScaffoldId, PolicyId, ProbeId, QueueId, RoleId, ScheduleId,
    SeederId, ValidationRuleId,
};
pub use websocket::{ERROR_EVENT, SUBSCRIBED_EVENT, SYSTEM_CHANNEL, UNSUBSCRIBED_EVENT};
