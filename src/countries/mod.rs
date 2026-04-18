use serde::{Deserialize, Serialize};

use crate::database::DbValue;
use crate::foundation::{AppContext, Error, Result};

const BUILTIN_SEED: &str = include_str!("seed.json");

/// Country activation status.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, forge_macros::AppEnum, ts_rs::TS, forge_macros::TS,
)]
#[ts(export)]
pub enum CountryStatus {
    Enabled,
    #[default]
    Disabled,
}

impl CountryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "enabled" => Self::Enabled,
            _ => Self::Disabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl std::fmt::Display for CountryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A country record from the `countries` table.
///
/// Primary key is `iso2` (2-letter ISO 3166-1 alpha-2 code), not a UUID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Country {
    pub iso2: String,
    pub iso3: String,
    pub iso_numeric: Option<String>,
    pub name: String,
    pub official_name: Option<String>,
    pub capital: Option<String>,
    pub region: Option<String>,
    pub subregion: Option<String>,
    pub currencies: serde_json::Value,
    pub primary_currency_code: Option<String>,
    pub calling_code: Option<String>,
    pub calling_root: Option<String>,
    pub calling_suffixes: serde_json::Value,
    pub tlds: serde_json::Value,
    pub timezones: serde_json::Value,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub independent: Option<bool>,
    pub un_member: Option<bool>,
    pub flag_emoji: Option<String>,
    pub status: CountryStatus,
    pub conversion_rate: Option<f64>,
}

impl Country {
    /// Find a country by ISO2 code.
    pub async fn find(app: &AppContext, iso2: &str) -> Result<Option<Country>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT * FROM countries WHERE iso2 = $1",
                &[DbValue::Text(iso2.to_ascii_uppercase())],
            )
            .await?;
        Ok(rows.first().map(row_to_country))
    }

    /// List all countries, ordered by name.
    pub async fn all(app: &AppContext) -> Result<Vec<Country>> {
        let db = app.database()?;
        let rows = db
            .raw_query("SELECT * FROM countries ORDER BY name", &[])
            .await?;
        Ok(rows.iter().map(row_to_country).collect())
    }

    /// List countries filtered by status.
    pub async fn by_status(app: &AppContext, status: CountryStatus) -> Result<Vec<Country>> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT * FROM countries WHERE status = $1 ORDER BY name",
                &[DbValue::Text(status.as_str().to_string())],
            )
            .await?;
        Ok(rows.iter().map(row_to_country).collect())
    }

    /// List only enabled countries.
    pub async fn enabled(app: &AppContext) -> Result<Vec<Country>> {
        Self::by_status(app, CountryStatus::Enabled).await
    }

    /// List only disabled countries.
    pub async fn disabled(app: &AppContext) -> Result<Vec<Country>> {
        Self::by_status(app, CountryStatus::Disabled).await
    }

    /// Check if an ISO2 code is valid (exists in the table).
    pub async fn exists(app: &AppContext, iso2: &str) -> Result<bool> {
        let db = app.database()?;
        let rows = db
            .raw_query(
                "SELECT 1 FROM countries WHERE iso2 = $1",
                &[DbValue::Text(iso2.to_ascii_uppercase())],
            )
            .await?;
        Ok(!rows.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Seed data
// ---------------------------------------------------------------------------

/// A country seed record from the built-in JSON data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountrySeed {
    pub iso2: String,
    pub iso3: String,
    #[serde(default)]
    pub iso_numeric: Option<String>,
    pub name: String,
    #[serde(default)]
    pub official_name: Option<String>,
    #[serde(default)]
    pub capital: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub subregion: Option<String>,
    #[serde(default)]
    pub currencies: Vec<CountryCurrency>,
    #[serde(default)]
    pub primary_currency_code: Option<String>,
    #[serde(default)]
    pub calling_code: Option<String>,
    #[serde(default)]
    pub calling_root: Option<String>,
    #[serde(default)]
    pub calling_suffixes: Vec<String>,
    #[serde(default)]
    pub tlds: Vec<String>,
    #[serde(default)]
    pub timezones: Vec<String>,
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    #[serde(default)]
    pub independent: Option<bool>,
    #[serde(default)]
    pub un_member: Option<bool>,
    #[serde(default)]
    pub flag_emoji: Option<String>,
    #[serde(default, alias = "status")]
    pub assignment_status: Option<String>,
    #[serde(default)]
    pub capitals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountryCurrency {
    pub code: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub minor_units: Option<i16>,
}

/// Load the built-in 250 country seed records.
pub fn load_seed() -> Result<Vec<CountrySeed>> {
    serde_json::from_str(BUILTIN_SEED)
        .map_err(|e| Error::message(format!("failed to parse built-in countries seed: {e}")))
}

/// Seed the countries table from built-in data.
///
/// Uses upsert (ON CONFLICT iso2 DO UPDATE) so it's safe to run multiple times.
pub async fn seed_countries(app: &AppContext) -> Result<u64> {
    let db = app.database()?;
    let seeds = load_seed()?;
    let mut count = 0u64;

    for seed in seeds {
        let iso2 = seed.iso2.trim().to_ascii_uppercase();
        let iso3 = seed.iso3.trim().to_ascii_uppercase();
        let currencies = serde_json::to_value(&seed.currencies).unwrap_or_default();
        let calling_suffixes = serde_json::to_value(&seed.calling_suffixes).unwrap_or_default();
        let tlds = serde_json::to_value(&seed.tlds).unwrap_or_default();
        let timezones = serde_json::to_value(&seed.timezones).unwrap_or_default();

        db.raw_execute(
            "INSERT INTO countries (iso2, iso3, iso_numeric, name, official_name, capital, region, subregion, \
             currencies, primary_currency_code, calling_code, calling_root, calling_suffixes, tlds, timezones, \
             latitude, longitude, independent, un_member, flag_emoji, status, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, 'disabled', NOW()) \
             ON CONFLICT (iso2) DO UPDATE SET \
             iso3 = $2, iso_numeric = $3, name = $4, official_name = $5, capital = $6, region = $7, subregion = $8, \
             currencies = $9, primary_currency_code = $10, calling_code = $11, calling_root = $12, \
             calling_suffixes = $13, tlds = $14, timezones = $15, latitude = $16, longitude = $17, \
             independent = $18, un_member = $19, flag_emoji = $20, updated_at = NOW()",
            &[
                DbValue::Text(iso2),
                DbValue::Text(iso3),
                opt_text(&seed.iso_numeric),
                DbValue::Text(seed.name.trim().to_string()),
                opt_text(&seed.official_name),
                opt_text(&seed.capital),
                opt_text(&seed.region),
                opt_text(&seed.subregion),
                DbValue::Json(currencies),
                opt_text(&seed.primary_currency_code),
                opt_text(&seed.calling_code),
                opt_text(&seed.calling_root),
                DbValue::Json(calling_suffixes),
                DbValue::Json(tlds),
                DbValue::Json(timezones),
                opt_f64(seed.latitude),
                opt_f64(seed.longitude),
                opt_bool(seed.independent),
                opt_bool(seed.un_member),
                opt_text(&seed.flag_emoji),
            ],
        )
        .await?;
        count += 1;
    }

    Ok(count)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn opt_text(value: &Option<String>) -> DbValue {
    match value {
        Some(s) if !s.trim().is_empty() => DbValue::Text(s.trim().to_string()),
        _ => DbValue::Null(crate::database::DbType::Text),
    }
}

fn opt_f64(value: Option<f64>) -> DbValue {
    match value {
        Some(v) => DbValue::Float64(v),
        None => DbValue::Null(crate::database::DbType::Float64),
    }
}

fn opt_bool(value: Option<bool>) -> DbValue {
    match value {
        Some(v) => DbValue::Bool(v),
        None => DbValue::Null(crate::database::DbType::Bool),
    }
}

fn row_to_country(row: &crate::database::DbRecord) -> Country {
    Country {
        iso2: row.text("iso2"),
        iso3: row.text("iso3"),
        iso_numeric: row.optional_text("iso_numeric"),
        name: row.text("name"),
        official_name: row.optional_text("official_name"),
        capital: row.optional_text("capital"),
        region: row.optional_text("region"),
        subregion: row.optional_text("subregion"),
        currencies: match row.get("currencies") {
            Some(DbValue::Json(v)) => v.clone(),
            _ => serde_json::json!([]),
        },
        primary_currency_code: row.optional_text("primary_currency_code"),
        calling_code: row.optional_text("calling_code"),
        calling_root: row.optional_text("calling_root"),
        calling_suffixes: match row.get("calling_suffixes") {
            Some(DbValue::Json(v)) => v.clone(),
            _ => serde_json::json!([]),
        },
        tlds: match row.get("tlds") {
            Some(DbValue::Json(v)) => v.clone(),
            _ => serde_json::json!([]),
        },
        timezones: match row.get("timezones") {
            Some(DbValue::Json(v)) => v.clone(),
            _ => serde_json::json!([]),
        },
        latitude: match row.get("latitude") {
            Some(DbValue::Float64(v)) => Some(*v),
            _ => None,
        },
        longitude: match row.get("longitude") {
            Some(DbValue::Float64(v)) => Some(*v),
            _ => None,
        },
        independent: match row.get("independent") {
            Some(DbValue::Bool(v)) => Some(*v),
            _ => None,
        },
        un_member: match row.get("un_member") {
            Some(DbValue::Bool(v)) => Some(*v),
            _ => None,
        },
        flag_emoji: row.optional_text("flag_emoji"),
        status: CountryStatus::parse(&row.text("status")),
        conversion_rate: match row.get("conversion_rate") {
            Some(DbValue::Float64(v)) => Some(*v),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_seed_parses_all_250_countries() {
        let countries = load_seed().unwrap();
        assert_eq!(countries.len(), 250);
    }

    #[test]
    fn seed_data_has_expected_countries() {
        let countries = load_seed().unwrap();
        let my = countries.iter().find(|c| c.iso2 == "MY").unwrap();
        assert_eq!(my.name, "Malaysia");
        assert_eq!(my.iso3, "MYS");
        assert!(my.flag_emoji.is_some());
    }

    #[test]
    fn seed_data_has_currencies() {
        let countries = load_seed().unwrap();
        let us = countries.iter().find(|c| c.iso2 == "US").unwrap();
        assert!(!us.currencies.is_empty());
        assert_eq!(us.currencies[0].code, "USD");
    }
}
