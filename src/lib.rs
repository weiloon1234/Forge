extern crate self as forge;

#[doc(hidden)]
pub mod __reexports {
    pub use async_trait::async_trait;
}

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

pub mod app_enum;
pub mod attachments;
pub mod audit;
pub mod auth;
pub mod cache;
pub mod cli;
pub mod config;
pub mod countries;
pub mod database;
pub mod datatable;
pub mod email;
pub mod events;
pub mod foundation;
pub mod http;
pub mod i18n;
pub mod imaging;
pub mod jobs;
pub mod kernel;
pub mod logging;
pub mod metadata;
pub mod notifications;
pub mod openapi;
pub mod plugin;
pub mod prelude;
pub mod redis;
pub mod scheduler;
pub mod settings;
pub mod storage;
pub mod support;
pub mod testing;
pub mod translations;
pub mod typescript;
pub mod validation;
pub mod websocket;

pub use forge_macros::{ApiSchema, AppEnum, Model, Projection, Validate, TS};
pub use inventory;
pub use ts_rs;

pub use attachments::{Attachment, AttachmentUploadBuilder, HasAttachments};
pub use audit::AuditLog;
pub use auth::{
    email_verification::EmailVerificationManager,
    lockout::{
        LockoutError, LockoutStore, LoginLockedOutEvent, LoginThrottle, RuntimeLockoutStore,
    },
    mfa::{
        routes as mfa_routes, CodeRequest as MfaCodeRequest, EnrollChallenge, MfaDisabledEvent,
        MfaEnrolledEvent, MfaFactor, MfaFailedEvent, MfaManager, MfaVerifiedEvent,
        RecoveryCodesRequest, RecoveryCodesResponse, TotpFactor,
    },
    password_reset::PasswordResetManager,
    session::SessionManager,
    token::{
        HasToken, RefreshTokenRequest, TokenAuthenticator, TokenManager, TokenPair, TokenResponse,
        WsTokenResponse,
    },
    AccessScope, Actor, Auth, AuthError, AuthErrorCode, AuthManager, Authenticatable,
    AuthenticatableRegistry, AuthenticatedModel, Authorizer, BearerAuthenticator, CurrentActor,
    GuardedAccess, OptionalActor, Policy, StaticBearerAuthenticator,
};
pub use cache::{CacheManager, CacheStore};
pub use countries::Country;
pub use database::{
    belongs_to, has_many, has_one, many_to_many, AggregateExpr, AggregateFn, AggregateNode,
    AggregateProjection, AnyRelation, BinaryExpr, BinaryOperator, Case, Column, ColumnInfo,
    ColumnRef, ComparisonOp, Condition, CreateDraft, CreateManyModel, CreateModel, CreateRow, Cte,
    CursorInfo, CursorMeta, CursorPaginated, CursorPagination, DatabaseManager,
    DatabaseTransaction, DbRecord, DbRecordStream, DbType, DbValue, DeleteModel, Expr, FromDbValue,
    FromItem, FunctionCall, InsertSource, IntoColumnValue, IntoFieldValue, IntoLoadableRelation,
    JoinKind, JoinNode, JsonExprBuilder, Loaded, LockBehavior, LockClause, LockStrength,
    ManyToManyDef, MigrationContext, MigrationFile, Model, ModelBehavior, ModelCollectionExt,
    ModelCreatedEvent, ModelCreatingEvent, ModelDeletedEvent, ModelDeletingEvent,
    ModelFeatureSetting, ModelHookContext, ModelInstanceWriteExt, ModelLifecycle,
    ModelLifecycleSnapshot, ModelPrimaryKeyStrategy, ModelQuery, ModelUpdatedEvent,
    ModelUpdatingEvent, ModelWriteExecutor, NoModelLifecycle, Numeric, OnConflictAction,
    OnConflictNode, OnConflictTarget, OrderBy, OrderDirection, Paginated, PaginatedResponse,
    Pagination, PaginationLinks, PaginationMeta, PersistedModel, Projection, ProjectionField,
    ProjectionFieldInfo, ProjectionMeta, ProjectionQuery, Query, QueryAst, QueryBody,
    QueryExecutionOptions, QueryExecutor, RelationAggregateDef, RelationDef, RelationKind,
    RelationLoader, RelationNode, RestoreModel, SeederContext, SeederFile, SelectItem, SelectNode,
    SetOperator, Sql, TableMeta, TableRef, ToDbValue, UnaryExpr, UnaryOperator, UpdateDraft,
    UpdateModel, Window, WindowBuilder, WindowExpr, WindowFrame, WindowFrameBound,
    WindowFrameUnits, WindowSpec,
};
pub use email::{
    EmailAddress, EmailAttachment, EmailDriver, EmailMailer, EmailManager, EmailMessage,
    LogEmailDriver, MailgunEmailDriver, PostmarkEmailDriver, RenderedTemplate, ResendEmailDriver,
    SesEmailDriver, SmtpEmailDriver, TemplateRenderer,
};
pub use foundation::{
    App, AppBuilder, AppContext, AppTransaction, Container, Error, Result, ServiceProvider,
    ServiceRegistrar,
};
pub use http::cookie::{Cookie, CookieJar, SessionCookie};
pub use http::middleware::{
    Compression, Cors, Csrf, CsrfToken, ETag, MaintenanceMode, MaxBodySize, MiddlewareConfig,
    MiddlewareGroups, RateLimit, RateLimitBy, RateLimitWindow, RealIp, RequestTimeout,
    SecurityHeaders, TrustedProxy,
};
pub use http::resource::ApiResource;
pub use http::response::MessageResponse;
pub use http::routes::RouteRegistry;
pub use http::{
    HttpAuthorizeContext, HttpRegistrar, HttpResourceRoutes, HttpRouteBuilder, HttpRouteOptions,
    HttpScope, JsonValidated, Validated,
};
pub use i18n::{I18n, I18nManager, Locale};
pub use imaging::{ImageFormat, ImageProcessor, Rotation};
pub use jobs::{spawn_worker, JobDeadLetterContext, JobHistoryStatus, JobMiddleware};
pub use kernel::worker::WorkerKernel;
pub use logging::{
    AuthOutcome, CurrentRequest, ErrorReporter, HandlerErrorReport, HttpOutcomeClass,
    JobDeadLetteredReport, JobOutcome, LivenessReport, LogFormat, LogLevel, ObservabilityOptions,
    PanicContext, PanicReport, ProbeResult, ProbeState, ReadinessCheck, ReadinessReport, RequestId,
    RuntimeBackendKind, RuntimeDiagnostics, RuntimeSnapshot, SchedulerLeadershipState,
    WebSocketConnectionState,
};
pub use metadata::{HasMetadata, ModelMeta};
pub use notifications::{
    BroadcastNotificationChannel, DatabaseNotificationChannel, EmailNotificationChannel,
    Notifiable, Notification, NotificationChannel, NotificationChannelRegistry, NOTIFY_BROADCAST,
    NOTIFY_DATABASE, NOTIFY_EMAIL,
};
pub use openapi::spec::{generate_openapi_spec, DocumentedRoute};
pub use openapi::{ApiSchema, RouteDoc, SchemaRef};
pub use plugin::{
    Plugin, PluginAsset, PluginAssetKind, PluginDependency, PluginInstallOptions, PluginManifest,
    PluginRegistrar, PluginRegistry, PluginScaffold, PluginScaffoldOptions, PluginScaffoldVar,
};
pub use redis::{RedisChannel, RedisConnection, RedisKey, RedisManager};
pub use scheduler::{CronExpression, ScheduleOptions};
pub use storage::{
    LocalStorageAdapter, MultipartForm, S3StorageAdapter, StorageAdapter, StorageConfig,
    StorageDisk, StorageManager, StorageVisibility, StoredFile, UploadedFile,
};
pub use support::lock::{DistributedLock, LockGuard};
pub use support::{
    sanitize_html, sha256_hex, sha256_hex_str, strip_tags, ChannelEventId, ChannelId, Clock,
    Collection, CommandId, CryptManager, Date, DateTime, EventId, GuardId, HashManager, JobId,
    LocalDateTime, MigrationId, ModelId, NotificationChannelId, PermissionId, PluginAssetId,
    PluginId, PluginScaffoldId, PolicyId, ProbeId, QueueId, RoleId, ScheduleId, SeederId, Time,
    Timezone, Token, ValidationRuleId,
};
pub use testing::{
    assert_safe_to_wipe, Factory, FactoryBuilder, TestApp, TestClient, TestResponse,
};
pub use translations::{
    current_locale, HasTranslations, ModelTranslation, TranslatedFields, CURRENT_LOCALE,
};
pub use websocket::{ERROR_EVENT, SUBSCRIBED_EVENT, SYSTEM_CHANNEL, UNSUBSCRIBED_EVENT};

pub use datatable::{
    Datatable, DatatableColumn, DatatableColumnMeta, DatatableContext, DatatableExportAccepted,
    DatatableExportDelivery, DatatableFilterBinding, DatatableFilterField, DatatableFilterInput,
    DatatableFilterKind, DatatableFilterOp, DatatableFilterOption, DatatableFilterRow,
    DatatableFilterValue, DatatableFilterValueKind, DatatableJsonResponse, DatatableMapping,
    DatatablePaginationMeta, DatatableQuery, DatatableRegistry, DatatableRequest, DatatableSort,
    DatatableSortInput, DatatableValue, GeneratedDatatableExport,
};

pub use app_enum::{EnumKey, EnumKeyKind, EnumMeta, EnumOption, ForgeAppEnum};
