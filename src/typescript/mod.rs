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
        (ts_type.export_fn)(dir)
            .map_err(|e| Error::message(format!("ts export `{}`: {e}", ts_type.name)))?;
        names.push(ts_type.name);
    }

    // Rewrite AppEnum files entirely — ts-rs may generate wrong casing,
    // so we regenerate from ForgeAppEnum::options() which is always correct.
    for enum_vals in inventory::iter::<TsEnumValues> {
        let file_path = dir.join(format!("{}.ts", enum_vals.name));
        let values = (enum_vals.values_fn)();
        let type_union = values
            .iter()
            .map(|v| format!("\"{}\"", v))
            .collect::<Vec<_>>()
            .join(" | ");
        let array_items = values
            .iter()
            .map(|v| format!("\"{}\"", v))
            .collect::<Vec<_>>()
            .join(", ");
        let content = format!(
            "// Auto-generated from AppEnum. Do not edit.\n\n\
             export type {} = {};\n\n\
             export const {}Values: {}[] = [{}];\n",
            enum_vals.name, type_union, enum_vals.name, enum_vals.name, array_items
        );
        std::fs::write(&file_path, content).map_err(Error::other)?;
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::export_all;

    #[test]
    fn exports_framework_typescript_helpers() {
        let dir = tempdir().unwrap();
        export_all(dir.path()).unwrap();

        for file in [
            "DatatableJsonResponse.ts",
            "DatatableRequest.ts",
            "MessageResponse.ts",
            "RefreshTokenRequest.ts",
            "TokenPair.ts",
            "TokenResponse.ts",
        ] {
            assert!(
                dir.path().join(file).exists(),
                "expected generated TypeScript file: {file}"
            );
        }
    }
}
