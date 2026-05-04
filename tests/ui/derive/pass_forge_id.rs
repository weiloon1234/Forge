use forge::{ForgeId, GuardId, PermissionId};

#[derive(Clone, Copy, ForgeId)]
#[forge(id = GuardId, rename_all = "snake_case")]
enum Guard {
    Api,
    AdminPortal,
}

#[derive(Clone, Copy, ForgeId)]
#[forge(id = PermissionId)]
enum Ability {
    #[forge(value = "reports:view")]
    ReportsView,
    #[forge(value = "users:manage")]
    UsersManage,
}

fn main() {
    let api: GuardId = Guard::Api.into();
    let admin = Guard::AdminPortal.id();
    let reports: PermissionId = Ability::ReportsView.into();
    let users = Ability::UsersManage.id();

    assert_eq!(api.as_str(), "api");
    assert_eq!(admin.as_str(), "admin_portal");
    assert_eq!(reports.as_str(), "reports:view");
    assert_eq!(users.as_str(), "users:manage");
    assert_eq!(Ability::ReportsView.to_string(), "reports:view");
}
