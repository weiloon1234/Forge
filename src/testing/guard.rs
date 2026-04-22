use url::Url;

use crate::foundation::{Error, Result};

pub fn assert_safe_to_wipe(db_url: &str) -> Result<()> {
    if std::env::var("FORGE_ALLOW_TEST_DB_WIPE").ok().as_deref() == Some("1") {
        return Ok(());
    }

    let database_name = database_name_from_url(db_url);
    let Some(database_name) = database_name else {
        return Err(Error::message(
            "refusing to wipe test database: URL does not contain a database name. Rename the database to start with `test_`, end with `_test`, or start with `forge_test`, or set FORGE_ALLOW_TEST_DB_WIPE=1 to override.",
        ));
    };

    if is_safe_test_database_name(&database_name) {
        return Ok(());
    }

    Err(Error::message(format!(
        "refusing to wipe database `{database_name}`. Rename it to start with `test_`, end with `_test`, or start with `forge_test`, or set FORGE_ALLOW_TEST_DB_WIPE=1 to override."
    )))
}

fn database_name_from_url(db_url: &str) -> Option<String> {
    let parsed = Url::parse(db_url).ok()?;
    parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn is_safe_test_database_name(database_name: &str) -> bool {
    database_name.starts_with("test_")
        || database_name.ends_with("_test")
        || database_name.starts_with("forge_test")
}

#[cfg(test)]
mod tests {
    use super::assert_safe_to_wipe;

    #[test]
    fn rejects_non_test_database_name() {
        let error = assert_safe_to_wipe("postgres://user@localhost/myapp").unwrap_err();
        assert!(error
            .to_string()
            .contains("refusing to wipe database `myapp`"));
    }

    #[test]
    fn allows_test_suffix_database_name() {
        assert!(assert_safe_to_wipe("postgres://user@localhost/myapp_test").is_ok());
    }

    #[test]
    fn allows_explicit_override() {
        unsafe {
            std::env::set_var("FORGE_ALLOW_TEST_DB_WIPE", "1");
        }
        assert!(assert_safe_to_wipe("postgres://user@localhost/myapp").is_ok());
        unsafe {
            std::env::remove_var("FORGE_ALLOW_TEST_DB_WIPE");
        }
    }

    #[test]
    fn rejects_urls_without_database_segment() {
        let error = assert_safe_to_wipe("postgres://user@localhost").unwrap_err();
        assert!(error
            .to_string()
            .contains("does not contain a database name"));
    }
}
