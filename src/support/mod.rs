use std::future::Future;
use std::pin::Pin;

mod identifiers;
pub(crate) mod runtime;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub use identifiers::{
    ChannelEventId, ChannelId, CommandId, EventId, GuardId, JobId, MigrationId, PermissionId,
    PluginAssetId, PluginId, PluginScaffoldId, PolicyId, ProbeId, QueueId, RoleId, ScheduleId,
    SeederId, ValidationRuleId,
};

pub fn boxed<F, T>(future: F) -> BoxFuture<T>
where
    F: Future<Output = T> + Send + 'static,
{
    Box::pin(future)
}
