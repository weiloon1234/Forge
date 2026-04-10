use forge::prelude::*;

#[derive(Clone, Copy)]
pub enum AuthGuard {
    Api,
}

impl From<AuthGuard> for GuardId {
    fn from(value: AuthGuard) -> Self {
        match value {
            AuthGuard::Api => GuardId::new("api"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Ability {
    DashboardView,
    RealtimeChat,
}

impl From<Ability> for PermissionId {
    fn from(value: Ability) -> Self {
        match value {
            Ability::DashboardView => PermissionId::new("dashboard:view"),
            Ability::RealtimeChat => PermissionId::new("realtime:chat"),
        }
    }
}

pub const MOBILE_RULE: ValidationRuleId = ValidationRuleId::new("mobile");
pub const PING_COMMAND: CommandId = CommandId::new("ping");
pub const HEARTBEAT_SCHEDULE: ScheduleId = ScheduleId::new("heartbeat");
pub const CHAT_CHANNEL: ChannelId = ChannelId::new("chat");
pub const ECHO_EVENT: ChannelEventId = ChannelEventId::new("echo");
