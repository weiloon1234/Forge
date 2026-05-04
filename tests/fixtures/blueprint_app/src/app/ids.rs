use forge::prelude::*;

#[derive(Clone, Copy, ForgeId)]
#[forge(id = GuardId, rename_all = "snake_case")]
pub enum AuthGuard {
    Api,
}

#[derive(Clone, Copy, ForgeId)]
#[forge(id = PermissionId)]
pub enum Ability {
    #[forge(value = "dashboard:view")]
    DashboardView,
    #[forge(value = "realtime:chat")]
    RealtimeChat,
}

#[derive(Clone, Copy, ForgeId)]
#[forge(id = RouteId)]
pub enum Route {
    #[forge(value = "health")]
    Health,
    #[forge(value = "users.store")]
    UsersStore,
}

pub const MOBILE_RULE: ValidationRuleId = ValidationRuleId::new("mobile");
pub const PING_COMMAND: CommandId = CommandId::new("ping");
pub const HEARTBEAT_SCHEDULE: ScheduleId = ScheduleId::new("heartbeat");
pub const CHAT_CHANNEL: ChannelId = ChannelId::new("chat");
pub const ECHO_EVENT: ChannelEventId = ChannelEventId::new("echo");
