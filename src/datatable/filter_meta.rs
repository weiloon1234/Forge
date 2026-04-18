use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::app_enum::{EnumKey, ForgeAppEnum};
use crate::support::Collection;

// ---------------------------------------------------------------------------
// Filter kind
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DatatableFilterKind {
    Text,
    Select,
    Checkbox,
    Date,
    DateTime,
}

// ---------------------------------------------------------------------------
// Select option
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
pub struct DatatableFilterOption {
    pub value: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug, Default, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
struct DatatableFilterOptions {
    pub items: Vec<DatatableFilterOption>,
}

impl DatatableFilterOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Filter field
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
pub struct DatatableFilterField {
    pub name: String,
    pub kind: DatatableFilterKind,
    pub label: String,
    #[ts(optional)]
    pub placeholder: Option<String>,
    #[ts(optional)]
    pub help: Option<String>,
    pub nullable: bool,
    #[ts(as = "DatatableFilterOptions")]
    pub options: Collection<DatatableFilterOption>,
}

impl Serialize for DatatableFilterField {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct(
            "DatatableFilterField",
            5 + usize::from(self.placeholder.is_some()) + usize::from(self.help.is_some()),
        )?;

        state.serialize_field("name", &self.name)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("label", &self.label)?;
        if let Some(placeholder) = &self.placeholder {
            state.serialize_field("placeholder", placeholder)?;
        }
        if let Some(help) = &self.help {
            state.serialize_field("help", help)?;
        }
        state.serialize_field("nullable", &self.nullable)?;
        state.serialize_field("options", &self.options)?;
        state.end()
    }
}

impl DatatableFilterField {
    fn new(name: impl Into<String>, label: impl Into<String>, kind: DatatableFilterKind) -> Self {
        Self {
            name: name.into(),
            kind,
            label: label.into(),
            placeholder: None,
            help: None,
            nullable: false,
            options: Collection::new(),
        }
    }

    pub fn text(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Text)
    }

    pub fn select(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Select)
    }

    pub fn checkbox(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Checkbox)
    }

    pub fn date(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Date)
    }

    pub fn datetime(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::DateTime)
    }

    // -- builder helpers ---------------------------------------------------

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn options<I>(mut self, options: I) -> Self
    where
        I: Into<Collection<DatatableFilterOption>>,
    {
        self.options = options.into();
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    /// Create a select filter with options auto-populated from an `AppEnum`.
    ///
    /// Works with both string-backed (`{ Pending, Completed }`) and
    /// int-backed (`{ Pending = 0, Completed = 1 }`) enums.
    ///
    /// ```ignore
    /// DatatableFilterField::enum_select::<CountryStatus>("status", "Status")
    /// ```
    pub fn enum_select<E: ForgeAppEnum>(name: impl Into<String>, label: impl Into<String>) -> Self {
        let options: Vec<DatatableFilterOption> = E::options()
            .iter()
            .map(|opt| {
                let value = match &opt.value {
                    EnumKey::String(s) => s.clone(),
                    EnumKey::Int(i) => i.to_string(),
                };
                DatatableFilterOption::new(value.clone(), value)
            })
            .collect();

        Self::select(name, label).options(options)
    }
}

// ---------------------------------------------------------------------------
// Filter row (layout)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
pub struct DatatableFilterRow {
    pub fields: Vec<DatatableFilterField>,
}

impl DatatableFilterRow {
    pub fn single(field: DatatableFilterField) -> Self {
        Self {
            fields: vec![field],
        }
    }

    pub fn pair(left: DatatableFilterField, right: DatatableFilterField) -> Self {
        Self {
            fields: vec![left, right],
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::DatatableFilterField;

    #[test]
    fn serializes_optional_metadata_only_when_present() {
        let filter = DatatableFilterField::text("status", "Status");
        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({
                "name": "status",
                "kind": "text",
                "label": "Status",
                "nullable": false,
                "options": {
                    "items": []
                }
            })
        );

        let filter = DatatableFilterField::text("status", "Status")
            .placeholder("Search status")
            .help("Filters by status");
        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({
                "name": "status",
                "kind": "text",
                "label": "Status",
                "placeholder": "Search status",
                "help": "Filters by status",
                "nullable": false,
                "options": {
                    "items": []
                }
            })
        );
    }
}
