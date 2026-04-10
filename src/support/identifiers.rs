use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

macro_rules! typed_identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Cow<'static, str>);

        impl $name {
            pub const fn new(value: &'static str) -> Self {
                Self(Cow::Borrowed(value))
            }

            pub fn owned(value: impl Into<String>) -> Self {
                Self(Cow::Owned(value.into()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                self.as_str()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

typed_identifier!(GuardId);
typed_identifier!(PolicyId);
typed_identifier!(PermissionId);
typed_identifier!(RoleId);
typed_identifier!(ValidationRuleId);
typed_identifier!(ChannelId);
typed_identifier!(ChannelEventId);
typed_identifier!(JobId);
typed_identifier!(QueueId);
typed_identifier!(EventId);
typed_identifier!(CommandId);
typed_identifier!(ScheduleId);
typed_identifier!(ProbeId);
typed_identifier!(PluginId);
typed_identifier!(PluginAssetId);
typed_identifier!(PluginScaffoldId);
typed_identifier!(MigrationId);
typed_identifier!(SeederId);

#[cfg(test)]
mod tests {
    use super::{
        ChannelId, GuardId, MigrationId, PluginAssetId, PluginId, PluginScaffoldId, ProbeId,
        QueueId, SeederId,
    };

    #[test]
    fn identifiers_expose_static_values() {
        const API: GuardId = GuardId::new("api");
        const CHAT: ChannelId = ChannelId::new("chat");
        const READINESS: ProbeId = ProbeId::new("ready.database");
        const DEFAULT_QUEUE: QueueId = QueueId::new("default");
        const PLUGIN: PluginId = PluginId::new("forge.plugin");
        const ASSET: PluginAssetId = PluginAssetId::new("config");
        const SCAFFOLD: PluginScaffoldId = PluginScaffoldId::new("dashboard");
        const MIGRATION: MigrationId = MigrationId::new("202604091200_create_users");
        const SEEDER: SeederId = SeederId::new("users.seed");

        assert_eq!(API.as_str(), "api");
        assert_eq!(CHAT.as_str(), "chat");
        assert_eq!(READINESS.as_str(), "ready.database");
        assert_eq!(DEFAULT_QUEUE.as_str(), "default");
        assert_eq!(PLUGIN.as_str(), "forge.plugin");
        assert_eq!(ASSET.as_str(), "config");
        assert_eq!(SCAFFOLD.as_str(), "dashboard");
        assert_eq!(MIGRATION.as_str(), "202604091200_create_users");
        assert_eq!(SEEDER.as_str(), "users.seed");
    }
}
