use std::future::Future;
use std::pin::Pin;

mod collection;
mod crypt;
mod datetime;
mod hash;
pub(crate) mod hmac;
mod identifiers;
pub mod lock;
pub(crate) mod runtime;
mod sanitize;
pub(crate) mod sha256;
mod token;

pub use collection::Collection;
pub use crypt::CryptManager;
pub use datetime::{Clock, Date, DateTime, LocalDateTime, Time, Timezone};
pub use hash::HashManager;
pub use sanitize::{sanitize_html, strip_tags};
pub use sha256::{sha256_hex, sha256_hex_str};
pub use token::Token;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub use identifiers::{
    ChannelEventId, ChannelId, CommandId, EventId, GuardId, JobId, MigrationId, ModelId,
    NotificationChannelId, PermissionId, PluginAssetId, PluginId, PluginScaffoldId, PolicyId,
    ProbeId, QueueId, RoleId, ScheduleId, SeederId, ValidationRuleId,
};

pub fn boxed<F, T>(future: F) -> BoxFuture<T>
where
    F: Future<Output = T> + Send + 'static,
{
    Box::pin(future)
}
