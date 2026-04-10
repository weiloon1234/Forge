use async_trait::async_trait;
use forge::prelude::*;

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users", lifecycle = UserLifecycle)]
struct User {
    id: i64,
    #[forge(column = "user_email")]
    email: String,
    active: bool,
    metadata: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    nickname: Option<String>,
    merchants: Loaded<Vec<Merchant>>,
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "merchants")]
struct Merchant {
    id: i64,
}

struct UserLifecycle;

#[async_trait]
impl ModelLifecycle<User> for UserLifecycle {}

fn main() {
    let _ = User::ID;
    let _ = User::EMAIL;
    let _ = User::table_meta();
    let _ = User::create();
}
