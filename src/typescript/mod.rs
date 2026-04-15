//! TypeScript type auto-export.
//!
//! Types that derive `ApiSchema`, `AppEnum`, or `forge::TS` are automatically
//! registered for TypeScript export via the `inventory` crate.
//!
//! `AppEnum` types also export a runtime values array:
//! ```ts
//! export type CountryStatus = "enabled" | "disabled";
//! export const CountryStatusValues: CountryStatus[] = ["enabled", "disabled"];
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::CommandRegistrar;
use crate::foundation::{Error, Result};
use crate::support::CommandId;

const TYPES_EXPORT_COMMAND: CommandId = CommandId::new("types:export");

/// A registered TypeScript type exporter.
pub struct TsType {
    pub name: &'static str,
    pub export_fn: fn(&Path) -> std::result::Result<(), ts_rs::ExportError>,
}

inventory::collect!(TsType);

/// A registered AppEnum with runtime values for TypeScript export.
pub struct TsEnumValues {
    pub name: &'static str,
    pub values_fn: fn() -> Vec<String>,
}

inventory::collect!(TsEnumValues);

/// Export all registered TypeScript types to a directory.
pub fn export_all(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(Error::other)?;

    // Clean existing .ts files
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let mut names: Vec<&str> = Vec::new();
    for ts_type in inventory::iter::<TsType> {
        (ts_type.export_fn)(dir).map_err(|e| Error::message(format!("ts export `{}`: {e}", ts_type.name)))?;
        names.push(ts_type.name);
    }

    // Append runtime values arrays for AppEnum types
    for enum_vals in inventory::iter::<TsEnumValues> {
        let file_path = dir.join(format!("{}.ts", enum_vals.name));
        if file_path.exists() {
            let values = (enum_vals.values_fn)();
            let values_str = values
                .iter()
                .map(|v| format!("\"{}\"", v))
                .collect::<Vec<_>>()
                .join(", ");
            let line = format!(
                "\nexport const {}Values: {}[] = [{}];\n",
                enum_vals.name, enum_vals.name, values_str
            );
            let mut content = std::fs::read_to_string(&file_path).map_err(Error::other)?;
            content.push_str(&line);
            std::fs::write(&file_path, content).map_err(Error::other)?;
        }
    }

    names.sort();
    names.dedup();

    // Generate barrel index.ts — export types + enum value constants
    let enum_names: Vec<&str> = inventory::iter::<TsEnumValues>().map(|e| e.name).collect();

    let mut barrel = String::from("// Auto-generated barrel. Do not edit.\n");
    for name in &names {
        if enum_names.contains(name) {
            barrel.push_str(&format!(
                "export {{ type {name}, {name}Values }} from \"./{name}\";\n"
            ));
        } else {
            barrel.push_str(&format!("export type {{ {name} }} from \"./{name}\";\n"));
        }
    }
    std::fs::write(dir.join("index.ts"), barrel).map_err(Error::other)?;

    println!("Exported {} type(s) to {}", names.len(), dir.display());

    Ok(())
}

/// CLI registrar for the `types:export` command.
pub fn builtin_cli_registrar() -> CommandRegistrar {
    Arc::new(|registry| {
        registry.command(
            TYPES_EXPORT_COMMAND,
            clap::Command::new("types:export")
                .about("Export registered TypeScript types")
                .arg(
                    clap::Arg::new("output")
                        .long("output")
                        .short('o')
                        .help("Output directory (overrides config)"),
                ),
            |invocation| async move {
                let output = if let Some(dir) = invocation.matches().get_one::<String>("output") {
                    PathBuf::from(dir)
                } else {
                    let config = invocation.app().config().typescript().unwrap_or_default();
                    PathBuf::from(config.output_dir)
                };

                export_all(&output)
            },
        )?;
        Ok(())
    })
}
