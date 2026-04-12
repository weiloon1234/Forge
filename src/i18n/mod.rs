pub mod extractor;

// Re-export the primary extractor type at module root for convenience.
pub use extractor::I18n;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::config::I18nConfig;
use crate::foundation::{Error, Result};

/// Translate a key using the [`I18n`] extractor with named parameters.
///
/// ```
/// use forge::prelude::*;
/// use forge::t;
///
/// async fn handler(i18n: I18n) -> String {
///     // No parameters
///     t!(i18n, "Something went wrong")
/// }
///
/// async fn greeting(i18n: I18n) -> String {
///     // Named parameters — order doesn't matter
///     t!(i18n, "Hello {{name2}} and {{name}}", name2 = "Alice", name = "Bob")
/// }
/// ```
#[macro_export]
macro_rules! t {
    ($i18n:expr, $key:expr) => {
        $i18n.t($key)
    };
    ($i18n:expr, $key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $i18n.t_with($key, &[$((stringify!($name), $value)),+])
    };
}

type Catalog = HashMap<String, String>;

/// Manages translation catalogs loaded at startup.
///
/// Scans `{resource_path}/{locale}/*.json`, merges all files per locale into
/// a single catalog, and provides O(1) translation lookups with a three-tier
/// fallback chain: requested locale → fallback locale → key itself.
///
/// Thread-safe by design — loaded once, never mutated.
pub struct I18nManager {
    default_locale: String,
    fallback_locale: String,
    catalogs: HashMap<String, Catalog>,
}

impl I18nManager {
    /// Load all translation catalogs from the configured resource path.
    ///
    /// Scans `{resource_path}/*/` for locale directories, reads all `*.json`
    /// files in each, and merges them into per-locale catalogs. Warns on
    /// duplicate keys (last file wins).
    pub fn load(config: &I18nConfig) -> Result<Self> {
        let resource_path = Path::new(&config.resource_path);

        if !resource_path.exists() {
            tracing::info!(
                "forge: i18n resource path not found, skipping: {}",
                config.resource_path
            );
            return Ok(Self {
                default_locale: config.default_locale.clone(),
                fallback_locale: config.fallback_locale.clone(),
                catalogs: HashMap::new(),
            });
        }

        let mut catalogs: HashMap<String, Catalog> = HashMap::new();

        let locale_dirs = fs::read_dir(resource_path)
            .map_err(Error::other)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir());

        for locale_dir in locale_dirs {
            let locale_name = match locale_dir.file_name().to_str() {
                Some(name) => name.to_string(),
                None => continue,
            };

            let mut catalog: Catalog = HashMap::new();

            let json_files = fs::read_dir(locale_dir.path())
                .map_err(Error::other)?
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("json"))
                        .unwrap_or(false)
                });

            for json_file in json_files {
                let content = fs::read_to_string(json_file.path()).map_err(Error::other)?;
                let value: Value = serde_json::from_str(&content).map_err(Error::other)?;

                if let Value::Object(map) = value {
                    merge_json_into_catalog(&mut catalog, &map, &locale_name);
                }
            }

            if !catalog.is_empty() {
                tracing::debug!(
                    "forge: i18n loaded {} keys for locale '{}'",
                    catalog.len(),
                    locale_name
                );
                catalogs.insert(locale_name, catalog);
            }
        }

        let loaded_locales: Vec<&str> = catalogs.keys().map(|s| s.as_str()).collect();
        tracing::info!("forge: i18n loaded locales: {:?}", loaded_locales);

        Ok(Self {
            default_locale: config.default_locale.clone(),
            fallback_locale: config.fallback_locale.clone(),
            catalogs,
        })
    }

    /// Translate a key in the given locale, interpolating values.
    ///
    /// Fallback chain:
    /// 1. `catalogs[locale][key]`
    /// 2. `catalogs[fallback_locale][key]`
    /// 3. `key` itself (the English string is the key)
    pub fn translate(&self, locale: &str, key: &str, values: &[(&str, &str)]) -> String {
        let template = self
            .catalogs
            .get(locale)
            .and_then(|cat| cat.get(key))
            .or_else(|| {
                self.catalogs
                    .get(&self.fallback_locale)
                    .and_then(|cat| cat.get(key))
            })
            .map(|s| s.as_str())
            .unwrap_or(key);

        if values.is_empty() {
            template.to_string()
        } else {
            interpolate(template, values)
        }
    }

    /// Resolve the best matching locale from an `Accept-Language` header value.
    ///
    /// Parses the header, finds the first locale that matches a loaded catalog,
    /// or falls back to the default locale.
    pub fn resolve_locale(&self, accept_language: &str) -> String {
        for tag in parse_accept_language(accept_language) {
            if self.has_locale(&tag) {
                return tag;
            }
        }
        self.default_locale.clone()
    }

    /// The configured default locale.
    pub fn default_locale(&self) -> &str {
        &self.default_locale
    }

    /// Whether a catalog exists for the given locale.
    pub fn has_locale(&self, locale: &str) -> bool {
        self.catalogs.contains_key(locale)
    }

    /// List of all loaded locale names.
    pub fn locale_list(&self) -> Vec<&str> {
        self.catalogs.keys().map(|s| s.as_str()).collect()
    }
}

/// Per-request locale stored in request extensions.
///
/// Can be set by custom middleware (e.g., from a cookie or user preference)
/// and is read by the `I18n` extractor.
#[derive(Clone, Debug)]
pub struct Locale(pub String);

/// Merge a JSON object (potentially nested) into a flat catalog.
///
/// Nested keys are flattened by joining with `.`:
/// `{"errors": {"not_found": "Not found"}}` → `"errors.not_found" → "Not found"`
///
/// Top-level string values are merged directly. Non-string leaf values are skipped.
fn merge_json_into_catalog(
    catalog: &mut Catalog,
    map: &serde_json::Map<String, Value>,
    locale: &str,
) {
    for (key, value) in map {
        match value {
            Value::String(s) => {
                if let Some(existing) = catalog.get(key) {
                    tracing::warn!(
                        "forge: i18n duplicate key '{}' in locale '{}', overwriting '{}' with '{}'",
                        key,
                        locale,
                        existing,
                        s
                    );
                }
                catalog.insert(key.clone(), s.clone());
            }
            Value::Object(nested) => {
                merge_json_nested(catalog, nested, key, locale);
            }
            _ => {}
        }
    }
}

fn merge_json_nested(
    catalog: &mut Catalog,
    map: &serde_json::Map<String, Value>,
    prefix: &str,
    locale: &str,
) {
    for (key, value) in map {
        let full_key = format!("{}.{}", prefix, key);
        match value {
            Value::String(s) => {
                if let Some(existing) = catalog.get(&full_key) {
                    tracing::warn!(
                        "forge: i18n duplicate key '{}' in locale '{}', overwriting '{}' with '{}'",
                        full_key,
                        locale,
                        existing,
                        s
                    );
                }
                catalog.insert(full_key, s.clone());
            }
            Value::Object(deeper) => {
                merge_json_nested(catalog, deeper, &full_key, locale);
            }
            _ => {}
        }
    }
}

/// Replace `{{var}}` placeholders with values.
fn interpolate(template: &str, values: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (key, value) in values {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, value);
    }
    result
}

/// Parse an `Accept-Language` header value into a list of locale tags.
///
/// Simple parsing: splits by `,`, strips quality values (`;q=...`),
/// trims whitespace. Does not implement full RFC 7231 quality sorting.
fn parse_accept_language(header: &str) -> Vec<String> {
    header
        .split(',')
        .filter_map(|tag| {
            let tag = tag.split(';').next()?.trim().to_string();
            if tag.is_empty() {
                None
            } else {
                Some(tag)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::config::I18nConfig;

    fn make_config(dir: &tempfile::TempDir) -> I18nConfig {
        I18nConfig {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            resource_path: dir.path().to_str().unwrap().to_string(),
        }
    }

    #[test]
    fn loads_catalogs_from_filesystem() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("en")).unwrap();
        fs::write(dir.path().join("en/common.json"), r#"{ "Hello": "Hello" }"#).unwrap();
        fs::create_dir(dir.path().join("ms")).unwrap();
        fs::write(dir.path().join("ms/common.json"), r#"{ "Hello": "Helo" }"#).unwrap();

        let manager = I18nManager::load(&make_config(&dir)).unwrap();

        assert_eq!(manager.translate("en", "Hello", &[]), "Hello");
        assert_eq!(manager.translate("ms", "Hello", &[]), "Helo");
    }

    #[test]
    fn merges_multiple_files_per_locale() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("en")).unwrap();
        fs::write(dir.path().join("en/common.json"), r#"{ "Hello": "Hello" }"#).unwrap();
        fs::write(
            dir.path().join("en/validation.json"),
            r#"{ "Required": "This field is required" }"#,
        )
        .unwrap();

        let manager = I18nManager::load(&make_config(&dir)).unwrap();

        assert_eq!(manager.translate("en", "Hello", &[]), "Hello");
        assert_eq!(
            manager.translate("en", "Required", &[]),
            "This field is required"
        );
    }

    #[test]
    fn falls_back_to_fallback_locale() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("en")).unwrap();
        fs::write(dir.path().join("en/common.json"), r#"{ "Hello": "Hello" }"#).unwrap();
        fs::create_dir(dir.path().join("ms")).unwrap();
        fs::write(dir.path().join("ms/common.json"), "{}").unwrap();

        let config = I18nConfig {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            resource_path: dir.path().to_str().unwrap().to_string(),
        };
        let manager = I18nManager::load(&config).unwrap();

        // "ms" locale doesn't have "Hello", falls back to "en"
        assert_eq!(manager.translate("ms", "Hello", &[]), "Hello");
    }

    #[test]
    fn returns_key_when_not_found_anywhere() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: HashMap::new(),
        };

        assert_eq!(manager.translate("en", "Missing key", &[]), "Missing key");
    }

    #[test]
    fn interpolates_values() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: {
                let mut m = HashMap::new();
                m.insert(
                    "en".to_string(),
                    HashMap::from([
                        ("Hello, {{name}}".to_string(), "Hello, {{name}}".to_string()),
                        ("{{count}} items".to_string(), "{{count}} items".to_string()),
                    ]),
                );
                m
            },
        };

        assert_eq!(
            manager.translate("en", "Hello, {{name}}", &[("name", "WeiLoon")]),
            "Hello, WeiLoon"
        );
        assert_eq!(
            manager.translate("en", "{{count}} items", &[("count", "5")]),
            "5 items"
        );
    }

    #[test]
    fn interpolates_translated_template() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: {
                let mut m = HashMap::new();
                m.insert(
                    "en".to_string(),
                    HashMap::from([("Hello, {{name}}".to_string(), "Hello, {{name}}".to_string())]),
                );
                m.insert(
                    "ms".to_string(),
                    HashMap::from([("Hello, {{name}}".to_string(), "Helo, {{name}}".to_string())]),
                );
                m
            },
        };

        assert_eq!(
            manager.translate("ms", "Hello, {{name}}", &[("name", "WeiLoon")]),
            "Helo, WeiLoon"
        );
    }

    #[test]
    fn resolves_locale_from_accept_language() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: {
                let mut m = HashMap::new();
                m.insert("en".to_string(), HashMap::new());
                m.insert("ms".to_string(), HashMap::new());
                m.insert("zh-CN".to_string(), HashMap::new());
                m
            },
        };

        assert_eq!(manager.resolve_locale("ms"), "ms");
        assert_eq!(manager.resolve_locale("ms,en-US;q=0.9"), "ms");
        assert_eq!(manager.resolve_locale("fr"), "en"); // not loaded, falls back
        assert_eq!(manager.resolve_locale("zh-CN,en;q=0.9"), "zh-CN");
    }

    #[test]
    fn flattens_nested_json() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("en")).unwrap();
        fs::write(
            dir.path().join("en/common.json"),
            r#"{
                "Something went wrong": "Something went wrong",
                "errors": {
                    "not_found": "Not found",
                    "validation": {
                        "required": "This field is required"
                    }
                }
            }"#,
        )
        .unwrap();

        let manager = I18nManager::load(&make_config(&dir)).unwrap();

        assert_eq!(
            manager.translate("en", "Something went wrong", &[]),
            "Something went wrong"
        );
        assert_eq!(
            manager.translate("en", "errors.not_found", &[]),
            "Not found"
        );
        assert_eq!(
            manager.translate("en", "errors.validation.required", &[]),
            "This field is required"
        );
    }

    #[test]
    fn handles_missing_resource_path_gracefully() {
        let config = I18nConfig {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            resource_path: "/nonexistent/path".to_string(),
        };

        let manager = I18nManager::load(&config).unwrap();
        assert_eq!(manager.translate("en", "Hello", &[]), "Hello");
    }

    #[test]
    fn parse_accept_language_basic() {
        let tags = parse_accept_language("en-US,en;q=0.9,ms;q=0.8");
        assert_eq!(tags, vec!["en-US", "en", "ms"]);
    }

    #[test]
    fn parse_accept_language_single() {
        let tags = parse_accept_language("ms");
        assert_eq!(tags, vec!["ms"]);
    }

    #[test]
    fn parse_accept_language_empty() {
        let tags = parse_accept_language("");
        assert!(tags.is_empty());
    }

    #[test]
    fn t_macro_no_params() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: {
                let mut m = HashMap::new();
                m.insert(
                    "en".to_string(),
                    HashMap::from([("Hello".to_string(), "Hello there".to_string())]),
                );
                m
            },
        };
        let i18n = crate::i18n::I18n::from_parts_for_test(
            "en".to_string(),
            Some(std::sync::Arc::new(manager)),
        );

        assert_eq!(t!(i18n, "Hello"), "Hello there");
    }

    #[test]
    fn t_macro_with_named_params() {
        let manager = I18nManager {
            default_locale: "en".to_string(),
            fallback_locale: "en".to_string(),
            catalogs: {
                let mut m = HashMap::new();
                m.insert(
                    "en".to_string(),
                    HashMap::from([(
                        "Hello {{name2}} and {{name}}".to_string(),
                        "Hello {{name2}} and {{name}}".to_string(),
                    )]),
                );
                m
            },
        };
        let i18n = crate::i18n::I18n::from_parts_for_test(
            "en".to_string(),
            Some(std::sync::Arc::new(manager)),
        );

        // Order doesn't matter — named params
        assert_eq!(
            t!(
                i18n,
                "Hello {{name2}} and {{name}}",
                name2 = "Alice",
                name = "Bob"
            ),
            "Hello Alice and Bob"
        );
    }

    #[test]
    fn t_macro_noop_when_no_manager() {
        let i18n = crate::i18n::I18n::from_parts_for_test("en".to_string(), None);

        assert_eq!(t!(i18n, "Missing key"), "Missing key");
        assert_eq!(t!(i18n, "Hello {{name}}", name = "World"), "Hello {{name}}");
    }
}
