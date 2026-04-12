use async_trait::async_trait;
use forge::prelude::*;

pub struct Entry;

#[async_trait]
impl SeederFile for Entry {
    async fn run(ctx: &SeederContext<'_>) -> Result<()> {
        let seeds = forge::countries::load_seed()?;
        let mut count = 0u64;

        for seed in seeds {
            let iso2 = seed.iso2.trim().to_ascii_uppercase();
            let iso3 = seed.iso3.trim().to_ascii_uppercase();
            let currencies = serde_json::to_value(&seed.currencies).unwrap_or_default();
            let calling_suffixes =
                serde_json::to_value(&seed.calling_suffixes).unwrap_or_default();
            let tlds = serde_json::to_value(&seed.tlds).unwrap_or_default();
            let timezones = serde_json::to_value(&seed.timezones).unwrap_or_default();

            ctx.raw_execute(
                r#"
                INSERT INTO countries (iso2, iso3, iso_numeric, name, official_name, capital,
                    region, subregion, currencies, primary_currency_code, calling_code,
                    calling_root, calling_suffixes, tlds, timezones, latitude, longitude,
                    independent, un_member, flag_emoji, status, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                    $16, $17, $18, $19, $20, 'disabled', NOW())
                ON CONFLICT (iso2) DO UPDATE SET
                    iso3 = $2, iso_numeric = $3, name = $4, official_name = $5,
                    capital = $6, region = $7, subregion = $8, currencies = $9,
                    primary_currency_code = $10, calling_code = $11, calling_root = $12,
                    calling_suffixes = $13, tlds = $14, timezones = $15, latitude = $16,
                    longitude = $17, independent = $18, un_member = $19, flag_emoji = $20,
                    updated_at = NOW()
                "#,
                &[
                    forge::database::DbValue::Text(iso2),
                    forge::database::DbValue::Text(iso3),
                    opt_text(&seed.iso_numeric),
                    forge::database::DbValue::Text(seed.name.trim().to_string()),
                    opt_text(&seed.official_name),
                    opt_text(&seed.capital),
                    opt_text(&seed.region),
                    opt_text(&seed.subregion),
                    forge::database::DbValue::Json(currencies),
                    opt_text(&seed.primary_currency_code),
                    opt_text(&seed.calling_code),
                    opt_text(&seed.calling_root),
                    forge::database::DbValue::Json(calling_suffixes),
                    forge::database::DbValue::Json(tlds),
                    forge::database::DbValue::Json(timezones),
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

        println!("  seeded {count} countries");
        Ok(())
    }
}

fn opt_text(value: &Option<String>) -> forge::database::DbValue {
    match value {
        Some(s) if !s.trim().is_empty() => {
            forge::database::DbValue::Text(s.trim().to_string())
        }
        _ => forge::database::DbValue::Null(forge::database::DbType::Text),
    }
}

fn opt_f64(value: Option<f64>) -> forge::database::DbValue {
    match value {
        Some(v) => forge::database::DbValue::Float64(v),
        None => forge::database::DbValue::Null(forge::database::DbType::Float64),
    }
}

fn opt_bool(value: Option<bool>) -> forge::database::DbValue {
    match value {
        Some(v) => forge::database::DbValue::Bool(v),
        None => forge::database::DbValue::Null(forge::database::DbType::Bool),
    }
}
