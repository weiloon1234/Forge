mod helpers;
mod r#trait;
mod types;

pub use helpers::{to_snake_case, to_title_text};
pub use r#trait::ForgeAppEnum;
pub use types::{EnumKey, EnumKeyKind, EnumMeta, EnumOption};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{DbType, DbValue, FromDbValue, ToDbValue};
    use crate::validation::{RuleRegistry, Validator};
    use crate::{config::ConfigRepository, foundation::Container};

    // -----------------------------------------------------------------------
    // Test enums
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
    enum OrderStatus {
        Pending,
        Reviewing,
        Completed,
    }

    #[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
    enum OrderStatusWithOverrides {
        Pending,
        #[forge(key = "in_review")]
        Reviewing,
        #[forge(label_key = "Order completed")]
        Completed,
    }

    #[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
    enum UserStatus {
        Pending = 0,
        Verified = 1,
        Suspended = 2,
    }

    #[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
    #[forge(id = "custom_status")]
    enum CustomIdEnum {
        Alpha,
        Beta,
    }

    #[derive(Clone, Debug, PartialEq, Eq, forge::AppEnum)]
    enum AliasedEnum {
        #[forge(aliases = ["awaiting", "queued"])]
        Pending,
        Active,
    }

    fn test_app() -> crate::foundation::AppContext {
        crate::foundation::AppContext::new(
            Container::new(),
            ConfigRepository::empty(),
            RuleRegistry::new(),
        )
        .unwrap()
    }

    // -----------------------------------------------------------------------
    // String-backed tests
    // -----------------------------------------------------------------------

    #[test]
    fn string_backed_key_returns_snake_case() {
        assert_eq!(OrderStatus::Pending.key(), EnumKey::String("pending".into()));
        assert_eq!(
            OrderStatus::Reviewing.key(),
            EnumKey::String("reviewing".into())
        );
        assert_eq!(
            OrderStatus::Completed.key(),
            EnumKey::String("completed".into())
        );
    }

    #[test]
    fn string_backed_parse_key_valid() {
        assert_eq!(OrderStatus::parse_key("pending"), Some(OrderStatus::Pending));
        assert_eq!(
            OrderStatus::parse_key("reviewing"),
            Some(OrderStatus::Reviewing)
        );
        assert_eq!(
            OrderStatus::parse_key("completed"),
            Some(OrderStatus::Completed)
        );
    }

    #[test]
    fn string_backed_parse_key_invalid() {
        assert_eq!(OrderStatus::parse_key("unknown"), None);
    }

    #[test]
    fn string_backed_keys_returns_all() {
        let keys = OrderStatus::keys();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn string_backed_label_key_default() {
        assert_eq!(OrderStatus::Pending.label_key(), "Pending");
        assert_eq!(OrderStatus::Reviewing.label_key(), "Reviewing");
        assert_eq!(OrderStatus::Completed.label_key(), "Completed");
    }

    #[test]
    fn string_backed_options() {
        let options = OrderStatus::options();
        assert_eq!(options.len(), 3);

        let opts: Vec<_> = options.into_iter().collect();
        assert_eq!(opts[0].value, EnumKey::String("pending".into()));
        assert_eq!(opts[0].label_key, "Pending");
        assert_eq!(opts[1].value, EnumKey::String("reviewing".into()));
        assert_eq!(opts[1].label_key, "Reviewing");
        assert_eq!(opts[2].value, EnumKey::String("completed".into()));
        assert_eq!(opts[2].label_key, "Completed");
    }

    #[test]
    fn string_backed_meta() {
        let meta = OrderStatus::meta();
        assert_eq!(meta.id, "order_status");
        assert_eq!(meta.key_kind, EnumKeyKind::String);
    }

    // -----------------------------------------------------------------------
    // Override tests
    // -----------------------------------------------------------------------

    #[test]
    fn override_key() {
        assert_eq!(
            OrderStatusWithOverrides::Reviewing.key(),
            EnumKey::String("in_review".into())
        );
    }

    #[test]
    fn override_parse_key() {
        assert_eq!(
            OrderStatusWithOverrides::parse_key("in_review"),
            Some(OrderStatusWithOverrides::Reviewing)
        );
    }

    #[test]
    fn override_label_key() {
        assert_eq!(
            OrderStatusWithOverrides::Completed.label_key(),
            "Order completed"
        );
    }

    // -----------------------------------------------------------------------
    // Int-backed tests
    // -----------------------------------------------------------------------

    #[test]
    fn int_backed_key_returns_int() {
        assert_eq!(UserStatus::Pending.key(), EnumKey::Int(0));
    }

    #[test]
    fn int_backed_parse_key_string() {
        assert_eq!(UserStatus::parse_key("0"), Some(UserStatus::Pending));
        assert_eq!(UserStatus::parse_key("1"), Some(UserStatus::Verified));
        assert_eq!(UserStatus::parse_key("2"), Some(UserStatus::Suspended));
    }

    #[test]
    fn int_backed_parse_key_invalid() {
        assert_eq!(UserStatus::parse_key("99"), None);
    }

    #[test]
    fn int_backed_key_kind() {
        assert_eq!(UserStatus::key_kind(), EnumKeyKind::Int);
    }

    #[test]
    fn int_backed_meta() {
        assert_eq!(UserStatus::meta().key_kind, EnumKeyKind::Int);
    }

    // -----------------------------------------------------------------------
    // Id tests
    // -----------------------------------------------------------------------

    #[test]
    fn id_inferred_from_type_name() {
        assert_eq!(OrderStatus::id(), "order_status");
    }

    #[test]
    fn id_explicit_override() {
        assert_eq!(CustomIdEnum::id(), "custom_status");
    }

    // -----------------------------------------------------------------------
    // DB_TYPE tests
    // -----------------------------------------------------------------------

    #[test]
    fn string_backed_db_type() {
        assert_eq!(OrderStatus::DB_TYPE, DbType::Text);
    }

    #[test]
    fn int_backed_db_type() {
        assert_eq!(UserStatus::DB_TYPE, DbType::Int32);
    }

    // -----------------------------------------------------------------------
    // ToDbValue tests
    // -----------------------------------------------------------------------

    #[test]
    fn to_db_value_string_backed() {
        assert_eq!(
            OrderStatus::Pending.to_db_value(),
            DbValue::Text("pending".into())
        );
    }

    #[test]
    fn to_db_value_int_backed() {
        assert_eq!(UserStatus::Verified.to_db_value(), DbValue::Int32(1));
    }

    // -----------------------------------------------------------------------
    // FromDbValue tests
    // -----------------------------------------------------------------------

    #[test]
    fn from_db_value_string_backed() {
        let result: crate::foundation::Result<OrderStatus> =
            FromDbValue::from_db_value(&DbValue::Text("pending".into()));
        assert_eq!(result.unwrap(), OrderStatus::Pending);
    }

    #[test]
    fn from_db_value_int_backed() {
        let result: crate::foundation::Result<UserStatus> =
            FromDbValue::from_db_value(&DbValue::Int32(1));
        assert_eq!(result.unwrap(), UserStatus::Verified);
    }

    #[test]
    fn from_db_value_invalid() {
        let result: std::result::Result<OrderStatus, _> =
            FromDbValue::from_db_value(&DbValue::Text("unknown".into()));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Serde tests
    // -----------------------------------------------------------------------

    #[test]
    fn serde_string_backed_serialize() {
        let json = serde_json::to_string(&OrderStatus::Pending).unwrap();
        assert_eq!(json, "\"pending\"");
    }

    #[test]
    fn serde_string_backed_deserialize_valid() {
        let result: OrderStatus = serde_json::from_str("\"pending\"").unwrap();
        assert_eq!(result, OrderStatus::Pending);
    }

    #[test]
    fn serde_string_backed_deserialize_invalid() {
        let result = serde_json::from_str::<OrderStatus>("\"unknown\"");
        assert!(result.is_err());
    }

    #[test]
    fn serde_int_backed_serialize() {
        let json = serde_json::to_string(&UserStatus::Pending).unwrap();
        assert_eq!(json, "0");
    }

    #[test]
    fn serde_int_backed_deserialize_valid() {
        let result: UserStatus = serde_json::from_str("1").unwrap();
        assert_eq!(result, UserStatus::Verified);
    }

    #[test]
    fn serde_int_backed_deserialize_invalid() {
        let result = serde_json::from_str::<UserStatus>("99");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Alias tests
    // -----------------------------------------------------------------------

    #[test]
    fn alias_parse_key() {
        assert_eq!(AliasedEnum::parse_key("awaiting"), Some(AliasedEnum::Pending));
        assert_eq!(AliasedEnum::parse_key("queued"), Some(AliasedEnum::Pending));
    }

    #[test]
    fn alias_key_returns_canonical() {
        assert_eq!(AliasedEnum::Pending.key(), EnumKey::String("pending".into()));
    }

    #[test]
    fn alias_parse_canonical_still_works() {
        assert_eq!(AliasedEnum::parse_key("pending"), Some(AliasedEnum::Pending));
    }

    // -----------------------------------------------------------------------
    // Validation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn validation_accepts_valid_key() {
        let app = test_app();
        let mut v = Validator::new(app);
        v.field("status", "pending")
            .app_enum::<OrderStatus>()
            .apply()
            .await
            .unwrap();
        assert!(v.finish().is_ok());
    }

    #[tokio::test]
    async fn validation_rejects_invalid_key() {
        let app = test_app();
        let mut v = Validator::new(app);
        v.field("status", "unknown")
            .app_enum::<OrderStatus>()
            .apply()
            .await
            .unwrap();
        let errors = v.finish().unwrap_err();
        assert_eq!(errors.errors[0].code, "app_enum");
    }
}
