use crate::database::DbType;
use crate::support::Collection;
use super::types::{EnumKey, EnumKeyKind, EnumMeta, EnumOption};

pub trait ForgeAppEnum: Sized + Clone + Send + Sync + 'static {
    /// The database type this enum stores as.
    /// Text for string-backed, Int32 for int-backed.
    const DB_TYPE: DbType;

    /// The enum identifier (for metadata/export grouping).
    /// Defaults to the type name in snake_case, can be overridden with #[forge(id = "...")].
    fn id() -> &'static str;

    /// Get the stored key for this variant.
    fn key(self) -> EnumKey;

    /// All valid keys for this enum.
    fn keys() -> Collection<EnumKey>;

    /// Parse a string key into the enum variant.
    /// For string-backed: matches against stored string keys.
    /// For int-backed: parses string as i32, then matches discriminants.
    /// Also matches any declared aliases.
    fn parse_key(key: &str) -> Option<Self>;

    /// Get the label key for this variant.
    /// Default is human-readable title text from variant name.
    /// Can be overridden with #[forge(label_key = "...")].
    fn label_key(self) -> &'static str;

    /// All options as value + label_key pairs.
    fn options() -> Collection<EnumOption>;

    /// Full metadata for this enum (export-ready).
    fn meta() -> EnumMeta;

    /// The key kind (String or Int).
    fn key_kind() -> EnumKeyKind;
}
