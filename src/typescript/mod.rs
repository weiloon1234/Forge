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
    use std::fs;

    use tempfile::tempdir;

    use super::export_all;

    #[test]
    fn exports_framework_typescript_helpers() {
        let dir = tempdir().unwrap();
        export_all(dir.path()).unwrap();

        for file in [
            "DatatableFilterBinding.ts",
            "DatatableFilterField.ts",
            "DatatableFilterValueKind.ts",
            "DatatableJsonResponse.ts",
            "DatatableRequest.ts",
            "MessageResponse.ts",
            "RefreshTokenRequest.ts",
            "TokenPair.ts",
            "TokenResponse.ts",
            "WsTokenResponse.ts",
        ] {
            assert!(
                dir.path().join(file).exists(),
                "expected generated TypeScript file: {file}"
            );
        }

        let datatable_filter_field =
            fs::read_to_string(dir.path().join("DatatableFilterField.ts")).unwrap();
        assert!(
            datatable_filter_field.contains("import type { DatatableFilterBinding } from \"./DatatableFilterBinding\";"),
            "expected DatatableFilterField.ts to import DatatableFilterBinding:\n{datatable_filter_field}"
        );
        assert!(
            datatable_filter_field.contains("import type { DatatableFilterOptions } from \"./DatatableFilterOptions\";"),
            "expected DatatableFilterField.ts to import DatatableFilterOptions:\n{datatable_filter_field}"
        );
        assert!(
            datatable_filter_field.contains("binding: DatatableFilterBinding"),
            "expected DatatableFilterField.ts to expose binding metadata:\n{datatable_filter_field}"
        );

        let datatable_filter_options =
            fs::read_to_string(dir.path().join("DatatableFilterOptions.ts")).unwrap();
        assert!(
            datatable_filter_options
                .contains("import type { DatatableFilterOption } from \"./DatatableFilterOption\";"),
            "expected DatatableFilterOptions.ts to import DatatableFilterOption:\n{datatable_filter_options}"
        );

        let datatable_filter_binding =
            fs::read_to_string(dir.path().join("DatatableFilterBinding.ts")).unwrap();
        assert!(
            datatable_filter_binding
                .contains("import type { DatatableFilterOp } from \"./DatatableFilterOp\";"),
            "expected DatatableFilterBinding.ts to import DatatableFilterOp:\n{datatable_filter_binding}"
        );
        assert!(
            datatable_filter_binding.contains(
                "import type { DatatableFilterValueKind } from \"./DatatableFilterValueKind\";"
            ),
            "expected DatatableFilterBinding.ts to import DatatableFilterValueKind:\n{datatable_filter_binding}"
        );
        assert!(
            datatable_filter_binding.contains("value_kind: DatatableFilterValueKind"),
            "expected DatatableFilterBinding.ts to expose value_kind:\n{datatable_filter_binding}"
        );

        let datatable_filter_kind =
            fs::read_to_string(dir.path().join("DatatableFilterKind.ts")).unwrap();
        assert!(
            datatable_filter_kind.contains("\"number\""),
            "expected DatatableFilterKind.ts to include number:\n{datatable_filter_kind}"
        );

        let datatable_filter_value_kind =
            fs::read_to_string(dir.path().join("DatatableFilterValueKind.ts")).unwrap();
        assert!(
            datatable_filter_value_kind.contains("\"decimal\""),
            "expected DatatableFilterValueKind.ts to include decimal:\n{datatable_filter_value_kind}"
        );

        let datatable_request = fs::read_to_string(dir.path().join("DatatableRequest.ts")).unwrap();
        assert!(
            datatable_request.contains("page: number"),
            "expected DatatableRequest.ts page field to use number:\n{datatable_request}"
        );
        assert!(
            datatable_request.contains("per_page: number"),
            "expected DatatableRequest.ts per_page field to use number:\n{datatable_request}"
        );
        assert!(
            !datatable_request.contains("bigint"),
            "did not expect bigint in DatatableRequest.ts:\n{datatable_request}"
        );

        let datatable_filter_value =
            fs::read_to_string(dir.path().join("DatatableFilterValue.ts")).unwrap();
        assert!(
            datatable_filter_value.contains("{ \"number\": number }"),
            "expected DatatableFilterValue::Number to use number:\n{datatable_filter_value}"
        );
        assert!(
            !datatable_filter_value.contains("bigint"),
            "did not expect bigint in DatatableFilterValue.ts:\n{datatable_filter_value}"
        );

        let datatable_json_response =
            fs::read_to_string(dir.path().join("DatatableJsonResponse.ts")).unwrap();
        assert!(
            datatable_json_response.contains("DatatablePaginationMeta"),
            "expected DatatableJsonResponse.ts to reference pagination metadata:\n{datatable_json_response}"
        );

        let datatable_pagination_meta =
            fs::read_to_string(dir.path().join("DatatablePaginationMeta.ts")).unwrap();
        assert!(
            datatable_pagination_meta.contains("page: number"),
            "expected DatatablePaginationMeta.ts page field to use number:\n{datatable_pagination_meta}"
        );
        assert!(
            datatable_pagination_meta.contains("total_pages: number"),
            "expected DatatablePaginationMeta.ts total_pages field to use number:\n{datatable_pagination_meta}"
        );
        assert!(
            !datatable_pagination_meta.contains("bigint"),
            "did not expect bigint in DatatablePaginationMeta.ts:\n{datatable_pagination_meta}"
        );

        let index = fs::read_to_string(dir.path().join("index.ts")).unwrap();
        assert!(
            index.contains("export type { WsTokenResponse } from \"./WsTokenResponse\";"),
            "expected index.ts to re-export WsTokenResponse:\n{index}"
        );
    }
}
