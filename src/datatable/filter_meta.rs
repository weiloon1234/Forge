use serde::Serialize;

use crate::app_enum::{EnumKey, ForgeAppEnum};
use crate::support::Collection;

// ---------------------------------------------------------------------------
// Filter kind
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DatatableFilterOption {
    pub value: String,
    pub label: String,
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

#[derive(Serialize, Clone, Debug)]
pub struct DatatableFilterField {
    pub name: String,
    pub kind: DatatableFilterKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    pub nullable: bool,
    #[serde(default)]
    pub options: Collection<DatatableFilterOption>,
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
                DatatableFilterOption::new(value, opt.label_key.clone())
            })
            .collect();

        Self::select(name, label).options(options)
    }
}

// ---------------------------------------------------------------------------
// Filter row (layout)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
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
