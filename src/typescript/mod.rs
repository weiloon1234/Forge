//! TypeScript type auto-export.
//!
//! Types that derive `ApiSchema`, `AppEnum`, or `forge::TS` are automatically
//! registered for TypeScript export via the `inventory` crate.
//!
//! Run `cargo run -- types:export` to export all registered types.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::CommandRegistrar;
use crate::foundation::{Error, Result};
use crate::support::CommandId;

const TYPES_EXPORT_COMMAND: CommandId = CommandId::new("types:export");
const DEFAULT_OUTPUT_DIR: &str = "frontend/shared/types/generated";

/// A registered TypeScript type exporter.
///
/// Created automatically by derive macros (`ApiSchema`, `AppEnum`, `TS`).
/// Collected at link time via `inventory`.
pub struct TsType {
    pub name: &'static str,
    pub export_fn: fn(&Path) -> std::result::Result<(), ts_rs::ExportError>,
}

inventory::collect!(TsType);

/// Export all registered TypeScript types to a directory.
///
/// Iterates all types registered via `inventory`, exports each to the
/// directory, and generates a barrel `index.ts` file.
pub fn export_all(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(Error::other)?;

    // Clean existing .ts files (avoid stale types)
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

    names.sort();
    names.dedup();

    // Generate barrel index.ts
    let mut barrel = String::from("// Auto-generated barrel. Do not edit.\n");
    for name in &names {
        barrel.push_str(&format!("export type {{ {name} }} from \"./{name}\";\n"));
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
                        .help("Output directory")
                        .default_value(DEFAULT_OUTPUT_DIR),
                ),
            |invocation| async move {
                let output = invocation
                    .matches()
                    .get_one::<String>("output")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_DIR));

                export_all(&output)
            },
        )?;
        Ok(())
    })
}
