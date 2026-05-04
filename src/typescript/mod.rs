//! TypeScript type auto-export.
//!
//! Types that derive `ApiSchema`, `AppEnum`, or `forge::TS` are automatically
//! registered for TypeScript export via the `inventory` crate.
//!
//! `AppEnum` types also export runtime metadata:
//! ```ts
//! export type CountryStatus = "enabled" | "disabled";
//! export const CountryStatusValues = ["enabled", "disabled"] as const;
//! export const CountryStatusOptions = [
//!   { value: "enabled", labelKey: "enum.country_status.enabled" },
//!   { value: "disabled", labelKey: "enum.country_status.disabled" },
//! ] as const;
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::app_enum::{EnumKey, EnumKeyKind, EnumMeta};
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

/// A registered AppEnum with runtime metadata for TypeScript export.
pub struct TsAppEnum {
    pub name: &'static str,
    pub meta_fn: fn() -> EnumMeta,
}

inventory::collect!(TsAppEnum);

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string literal serialization should not fail")
}

fn enum_key_kind_literal(kind: EnumKeyKind) -> &'static str {
    match kind {
        EnumKeyKind::String => "string",
        EnumKeyKind::Int => "int",
    }
}

fn enum_key_literal(value: &EnumKey) -> String {
    match value {
        EnumKey::String(value) => json_string(value),
        EnumKey::Int(value) => value.to_string(),
    }
}

fn render_array(items: &[String]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n  {},\n]", items.join(",\n  "))
    }
}

#[derive(Debug)]
struct RenderedAppEnum {
    content: String,
    has_groups: bool,
}

struct EnumGroup {
    property: String,
    actions: Vec<EnumGroupAction>,
}

struct EnumGroupAction {
    property: String,
    value: String,
}

fn is_ts_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn to_camel_case_identifier(value: &str) -> Result<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if matches!(ch, '_' | '-' | ' ') {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            return Err(Error::message(format!(
                "AppEnum grouped TypeScript export only supports ASCII module keys; `{value}` contains unsupported character `{ch}`"
            )));
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    if words.is_empty() {
        return Err(Error::message(
            "AppEnum grouped TypeScript export requires non-empty module keys",
        ));
    }

    let mut identifier = words[0].to_ascii_lowercase();
    for word in words.iter().skip(1) {
        let lower = word.to_ascii_lowercase();
        let mut chars = lower.chars();
        if let Some(first) = chars.next() {
            identifier.push(first.to_ascii_uppercase());
            identifier.push_str(chars.as_str());
        }
    }

    if !is_ts_identifier(&identifier) {
        return Err(Error::message(format!(
            "AppEnum grouped TypeScript export normalized module `{value}` to invalid TypeScript identifier `{identifier}`"
        )));
    }

    Ok(identifier)
}

fn parse_grouped_key<'a>(name: &str, key: &'a str) -> Result<Option<(&'a str, &'a str)>> {
    let mut parts = key.split('.');
    let module = parts.next().unwrap_or_default();
    let Some(action) = parts.next() else {
        return Ok(None);
    };

    if parts.next().is_some() || module.is_empty() || action.is_empty() {
        return Err(Error::message(format!(
            "AppEnum `{name}` grouped TypeScript export expects keys shaped `<module>.<action>`; got `{key}`"
        )));
    }

    if !is_ts_identifier(action) {
        return Err(Error::message(format!(
            "AppEnum `{name}` grouped TypeScript export requires action `{action}` from key `{key}` to be a TypeScript identifier"
        )));
    }

    Ok(Some((module, action)))
}

fn app_enum_groups(name: &str, meta: &EnumMeta) -> Result<Option<Vec<EnumGroup>>> {
    if meta.key_kind != EnumKeyKind::String {
        return Ok(None);
    }

    let mut saw_grouped = false;
    let mut saw_plain = false;
    let mut groups = Vec::<EnumGroup>::new();
    let mut module_properties = HashMap::<String, String>::new();

    for option in meta.options.iter() {
        let EnumKey::String(value) = &option.value else {
            continue;
        };

        let Some((module, action)) = parse_grouped_key(name, value)? else {
            saw_plain = true;
            continue;
        };

        saw_grouped = true;
        let module_property = to_camel_case_identifier(module)?;

        if let Some(existing) = module_properties.get(&module_property) {
            if existing != module {
                return Err(Error::message(format!(
                    "AppEnum `{name}` grouped TypeScript export has module keys `{existing}` and `{module}` that both normalize to `{module_property}`"
                )));
            }
        } else {
            module_properties.insert(module_property.clone(), module.to_string());
        }

        let group = if let Some(index) = groups
            .iter()
            .position(|group| group.property == module_property)
        {
            &mut groups[index]
        } else {
            groups.push(EnumGroup {
                property: module_property.clone(),
                actions: Vec::new(),
            });
            groups.last_mut().expect("just pushed group")
        };

        if group.actions.iter().any(|entry| entry.property == action) {
            return Err(Error::message(format!(
                "AppEnum `{name}` grouped TypeScript export has duplicate action `{action}` in module `{module}`"
            )));
        }

        group.actions.push(EnumGroupAction {
            property: action.to_string(),
            value: value.clone(),
        });
    }

    if saw_grouped && saw_plain {
        return Err(Error::message(format!(
            "AppEnum `{name}` grouped TypeScript export mixes dotted `<module>.<action>` keys with non-dotted keys"
        )));
    }

    if saw_grouped {
        Ok(Some(groups))
    } else {
        Ok(None)
    }
}

fn render_groups(name: &str, groups: &[EnumGroup]) -> String {
    let group_literals: Vec<String> = groups
        .iter()
        .map(|group| {
            let action_literals: Vec<String> = group
                .actions
                .iter()
                .map(|action| format!("{}: {}", action.property, json_string(&action.value)))
                .collect();

            format!("  {}: {{ {} }}", group.property, action_literals.join(", "))
        })
        .collect();

    format!(
        "\nexport const {name}Groups = {{\n{},\n}} as const;\n",
        group_literals.join(",\n")
    )
}

fn render_app_enum(name: &str, meta: &EnumMeta) -> Result<RenderedAppEnum> {
    let value_literals: Vec<String> = meta
        .options
        .iter()
        .map(|option| enum_key_literal(&option.value))
        .collect();
    let type_union = if value_literals.is_empty() {
        "never".to_string()
    } else {
        value_literals.join(" | ")
    };
    let option_literals: Vec<String> = meta
        .options
        .iter()
        .map(|option| {
            format!(
                "{{ value: {}, labelKey: {} }}",
                enum_key_literal(&option.value),
                json_string(&option.label_key),
            )
        })
        .collect();

    let groups = app_enum_groups(name, meta)?;
    let mut content = format!(
        "// Auto-generated from AppEnum. Do not edit.\n\n\
         export type {name} = {type_union};\n\n\
         export const {name}Values = {} as const;\n\n\
         export const {name}Options = {} as const;\n\n\
         export const {name}Meta = {{\n\
           id: {},\n\
           keyKind: {},\n\
           options: {name}Options,\n\
         }} as const;\n",
        render_array(&value_literals),
        render_array(&option_literals),
        json_string(&meta.id),
        json_string(enum_key_kind_literal(meta.key_kind)),
    );

    if let Some(groups) = &groups {
        content.push_str(&render_groups(name, groups));
    }

    Ok(RenderedAppEnum {
        content,
        has_groups: groups.is_some(),
    })
}

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

    // Rewrite AppEnum files entirely — if ts-rs also emitted an enum file,
    // the metadata-based AppEnum export owns the final file content.
    let mut enum_names = HashSet::new();
    let mut grouped_enum_names = HashSet::new();
    for app_enum in inventory::iter::<TsAppEnum> {
        let file_path = dir.join(format!("{}.ts", app_enum.name));
        let rendered = render_app_enum(app_enum.name, &(app_enum.meta_fn)())?;
        if rendered.has_groups {
            grouped_enum_names.insert(app_enum.name);
        }
        std::fs::write(&file_path, rendered.content).map_err(Error::other)?;
        enum_names.insert(app_enum.name);
        names.push(app_enum.name);
    }

    names.sort();
    names.dedup();

    let mut barrel = String::from("// Auto-generated barrel. Do not edit.\n");
    for name in &names {
        if enum_names.contains(name) {
            let groups_export = if grouped_enum_names.contains(name) {
                format!(", {name}Groups")
            } else {
                String::new()
            };
            barrel.push_str(&format!(
                "export {{ type {name}, {name}Values, {name}Options, {name}Meta{groups_export} }} from \"./{name}\";\n"
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

    use crate::app_enum::{EnumKey, EnumKeyKind, EnumMeta, EnumOption};
    use crate::support::Collection;

    use super::export_all;
    use super::render_app_enum;

    #[derive(Clone, Debug, PartialEq, Eq, crate::AppEnum)]
    enum MinimalExportStatus {
        Pending,
        Completed,
    }

    #[derive(Clone, Debug, PartialEq, Eq, crate::AppEnum)]
    enum MinimalExportPriority {
        Low = 1,
        High = 2,
    }

    #[derive(Clone, Debug, PartialEq, Eq, crate::AppEnum)]
    enum MinimalExportPermission {
        #[forge(key = "audit_logs.read")]
        AuditLogsRead,
        #[forge(key = "audit_logs.manage")]
        AuditLogsManage,
        #[forge(key = "observability.view")]
        ObservabilityView,
    }

    fn string_meta(values: &[&str]) -> EnumMeta {
        EnumMeta {
            id: "permission".to_string(),
            key_kind: EnumKeyKind::String,
            options: Collection::from(
                values
                    .iter()
                    .map(|value| EnumOption {
                        value: EnumKey::String((*value).to_string()),
                        label_key: format!("enum.permission.{value}"),
                    })
                    .collect::<Vec<_>>(),
            ),
        }
    }

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

        let minimal_status = fs::read_to_string(dir.path().join("MinimalExportStatus.ts")).unwrap();
        assert!(
            minimal_status
                .contains("export type MinimalExportStatus = \"pending\" | \"completed\";"),
            "expected MinimalExportStatus.ts to export a string union:\n{minimal_status}"
        );
        assert!(
            minimal_status.contains("export const MinimalExportStatusValues = ["),
            "expected MinimalExportStatus.ts to export Values:\n{minimal_status}"
        );
        assert!(
            minimal_status.contains(
                "{ value: \"pending\", labelKey: \"enum.minimal_export_status.pending\" }"
            ),
            "expected MinimalExportStatus.ts to export option metadata:\n{minimal_status}"
        );
        assert!(
            minimal_status.contains("keyKind: \"string\""),
            "expected MinimalExportStatus.ts to expose string keyKind:\n{minimal_status}"
        );

        let minimal_priority =
            fs::read_to_string(dir.path().join("MinimalExportPriority.ts")).unwrap();
        assert!(
            minimal_priority.contains("export type MinimalExportPriority = 1 | 2;"),
            "expected MinimalExportPriority.ts to export a numeric union:\n{minimal_priority}"
        );
        assert!(
            minimal_priority
                .contains("{ value: 1, labelKey: \"enum.minimal_export_priority.low\" }"),
            "expected MinimalExportPriority.ts to keep numeric option values:\n{minimal_priority}"
        );
        assert!(
            minimal_priority.contains("keyKind: \"int\""),
            "expected MinimalExportPriority.ts to expose int keyKind:\n{minimal_priority}"
        );

        let minimal_permission =
            fs::read_to_string(dir.path().join("MinimalExportPermission.ts")).unwrap();
        assert!(
            minimal_permission.contains("export const MinimalExportPermissionGroups = {"),
            "expected grouped AppEnum export:\n{minimal_permission}"
        );
        assert!(
            minimal_permission.contains(
                "auditLogs: { read: \"audit_logs.read\", manage: \"audit_logs.manage\" }"
            ),
            "expected snake_case modules to become camelCase groups:\n{minimal_permission}"
        );
        assert!(
            minimal_permission.contains("observability: { view: \"observability.view\" }"),
            "expected non-read/manage actions to stay available in groups:\n{minimal_permission}"
        );

        let index = fs::read_to_string(dir.path().join("index.ts")).unwrap();
        assert!(
            index.contains("export type { WsTokenResponse } from \"./WsTokenResponse\";"),
            "expected index.ts to re-export WsTokenResponse:\n{index}"
        );
        assert!(
            index.contains(
                "export { type MinimalExportStatus, MinimalExportStatusValues, MinimalExportStatusOptions, MinimalExportStatusMeta } from \"./MinimalExportStatus\";"
            ),
            "expected index.ts to re-export AppEnum metadata:\n{index}"
        );
        assert!(
            index.contains(
                "export { type MinimalExportPermission, MinimalExportPermissionValues, MinimalExportPermissionOptions, MinimalExportPermissionMeta, MinimalExportPermissionGroups } from \"./MinimalExportPermission\";"
            ),
            "expected index.ts to re-export AppEnum groups only for grouped enums:\n{index}"
        );
        assert!(
            !index.contains("MinimalExportStatusGroups"),
            "did not expect non-dotted AppEnum groups in barrel:\n{index}"
        );
    }

    #[test]
    fn app_enum_groups_are_not_rendered_for_plain_string_enums() {
        let rendered = render_app_enum("PlainStatus", &string_meta(&["pending", "completed"]))
            .expect("plain string enums should render");

        assert!(!rendered.has_groups);
        assert!(
            !rendered.content.contains("PlainStatusGroups"),
            "did not expect groups for non-dotted string enum:\n{}",
            rendered.content
        );
    }

    #[test]
    fn app_enum_groups_reject_mixed_dotted_and_plain_keys() {
        let error = render_app_enum("MixedPermission", &string_meta(&["users.read", "pending"]))
            .expect_err("mixed dotted and plain keys should fail");

        assert!(
            error
                .to_string()
                .contains("mixes dotted `<module>.<action>` keys with non-dotted keys"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn app_enum_groups_reject_camel_case_collisions() {
        let error = render_app_enum(
            "CollidingPermission",
            &string_meta(&["audit_logs.read", "audit-logs.manage"]),
        )
        .expect_err("camelCase module collisions should fail");

        assert!(
            error.to_string().contains("both normalize to `auditLogs`"),
            "unexpected error: {error}"
        );
    }
}
