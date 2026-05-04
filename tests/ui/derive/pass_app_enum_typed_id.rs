use forge::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, forge::AppEnum)]
#[forge(id = "ability", id_type = PermissionId)]
enum Ability {
    #[forge(key = "reports:view")]
    ReportsView,
    #[forge(key = "users:manage")]
    UsersManage,
}

fn main() {
    let reports: PermissionId = Ability::ReportsView.into();
    let users = Ability::UsersManage.typed_id();

    assert_eq!(Ability::id(), "ability");
    assert_eq!(reports.as_str(), "reports:view");
    assert_eq!(users.as_str(), "users:manage");
    assert_eq!(Ability::ReportsView.as_str(), "reports:view");
    assert_eq!(Ability::ReportsView.as_ref(), "reports:view");
    assert_eq!(Ability::UsersManage.to_string(), "users:manage");
}
