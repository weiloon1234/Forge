use async_trait::async_trait;
use forge::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UserStatus {
    Active,
    Disabled,
}

impl ToDbValue for UserStatus {
    fn to_db_value(self) -> DbValue {
        match self {
            Self::Active => "active".into(),
            Self::Disabled => "disabled".into(),
        }
    }
}

impl FromDbValue for UserStatus {
    fn from_db_value(value: &DbValue) -> Result<Self> {
        match value {
            DbValue::Text(value) if value == "active" => Ok(Self::Active),
            DbValue::Text(value) if value == "disabled" => Ok(Self::Disabled),
            _ => Err(Error::message("unknown user status")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, forge::Model)]
#[forge(model = "users", lifecycle = UserLifecycle)]
struct User {
    id: i64,
    email: String,
    #[forge(db_type = "text")]
    status: UserStatus,
    nickname: Option<String>,
}

struct UserLifecycle;

#[async_trait]
impl ModelLifecycle<User> for UserLifecycle {
    async fn creating(
        _context: &ModelHookContext<'_>,
        draft: &mut CreateDraft<User>,
    ) -> Result<()> {
        if draft.pending_record().get("nickname").is_none() {
            draft.set(User::NICKNAME, "new-user");
        }
        Ok(())
    }
}

async fn aggregate_examples(db: &DatabaseManager) -> Result<()> {
    let active_count = User::query()
        .where_(User::STATUS.eq(UserStatus::Active))
        .count(db)
        .await?;
    let id_sum = User::query().sum(db, User::ID).await?;

    println!("active users = {active_count}");
    println!("sum of user ids = {:?}", id_sum);

    Ok(())
}

async fn write_examples(app: &AppContext) -> Result<()> {
    let created = User::create()
        .set(User::ID, 1_i64)
        .set(User::EMAIL, "forge@example.com")
        .set(User::STATUS, UserStatus::Active)
        .save(app)
        .await?;

    let _updated = created
        .update()
        .set(User::NICKNAME, "captain")
        .save(app)
        .await?;

    User::delete()
        .where_(User::ID.eq(created.id))
        .execute(app)
        .await?;

    Ok(())
}

fn main() -> Result<()> {
    let list_users = User::query()
        .where_(User::STATUS.eq(UserStatus::Active))
        .order_by(User::ID.asc());
    let _insert_user = User::create()
        .set(User::ID, 1_i64)
        .set(User::EMAIL, "forge@example.com")
        .set(User::STATUS, UserStatus::Active)
        .set(User::NICKNAME, "captain");
    let _bulk_insert = User::create_many()
        .row(|row| {
            row.set(User::ID, 2_i64)
                .set(User::EMAIL, "ops@example.com")
                .set(User::STATUS, UserStatus::Disabled)
                .set(User::NICKNAME, None::<String>)
        })
        .row(|row| {
            row.set(User::ID, 3_i64)
                .set(User::EMAIL, "dev@example.com")
                .set(User::STATUS, UserStatus::Active)
                .set(User::NICKNAME, "ally")
        });
    let _upsert_user = User::create()
        .set(User::ID, 3_i64)
        .set(User::EMAIL, "dev-updated@example.com")
        .set(User::STATUS, UserStatus::Active)
        .set(User::NICKNAME, "vip")
        .on_conflict_columns([User::ID])
        .do_update()
        .set_excluded(User::EMAIL)
        .set_excluded(User::STATUS)
        .set_excluded(User::NICKNAME);
    let _patch_user = User::update()
        .set(User::STATUS, UserStatus::Disabled)
        .set_null(User::NICKNAME)
        .where_(User::ID.eq(3_i64));

    println!("{:?}", list_users.ast());
    let _ = aggregate_examples;
    let _ = write_examples;

    Ok(())
}
