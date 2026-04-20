use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::app_enum::{EnumKey, ForgeAppEnum};
use crate::support::Collection;

use super::request::DatatableFilterOp;

// ---------------------------------------------------------------------------
// Filter kind
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DatatableFilterKind {
    Text,
    Number,
    Select,
    Checkbox,
    Date,
    DateTime,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DatatableFilterValueKind {
    Text,
    Boolean,
    Integer,
    Decimal,
    Date,
    DateTime,
    Values,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
pub struct DatatableFilterBinding {
    pub field: String,
    pub op: DatatableFilterOp,
    pub value_kind: DatatableFilterValueKind,
}

impl DatatableFilterBinding {
    pub fn new(
        field: impl Into<String>,
        op: DatatableFilterOp,
        value_kind: DatatableFilterValueKind,
    ) -> Self {
        Self {
            field: field.into(),
            op,
            value_kind,
        }
    }
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

#[derive(Clone, Debug, PartialEq, Eq, ts_rs::TS, forge_macros::TS)]
#[ts(export)]
pub struct DatatableFilterField {
    pub name: String,
    pub kind: DatatableFilterKind,
    pub label: String,
    pub binding: DatatableFilterBinding,
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
            6 + usize::from(self.placeholder.is_some()) + usize::from(self.help.is_some()),
        )?;

        state.serialize_field("name", &self.name)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("label", &self.label)?;
        state.serialize_field("binding", &self.binding)?;
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
        let name = name.into();
        Self {
            binding: Self::default_binding(name.as_str(), kind),
            name,
            kind,
            label: label.into(),
            placeholder: None,
            help: None,
            nullable: false,
            options: Collection::new(),
        }
    }

    fn new_with_binding(
        name: impl Into<String>,
        label: impl Into<String>,
        kind: DatatableFilterKind,
        op: DatatableFilterOp,
        value_kind: DatatableFilterValueKind,
    ) -> Self {
        let name = name.into();
        Self {
            binding: DatatableFilterBinding::new(name.clone(), op, value_kind),
            name,
            kind,
            label: label.into(),
            placeholder: None,
            help: None,
            nullable: false,
            options: Collection::new(),
        }
    }

    fn default_binding(name: &str, kind: DatatableFilterKind) -> DatatableFilterBinding {
        match kind {
            DatatableFilterKind::Text => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Eq,
                DatatableFilterValueKind::Text,
            ),
            DatatableFilterKind::Number => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Eq,
                DatatableFilterValueKind::Integer,
            ),
            DatatableFilterKind::Select => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Eq,
                DatatableFilterValueKind::Text,
            ),
            DatatableFilterKind::Checkbox => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Eq,
                DatatableFilterValueKind::Boolean,
            ),
            DatatableFilterKind::Date => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Date,
                DatatableFilterValueKind::Date,
            ),
            DatatableFilterKind::DateTime => DatatableFilterBinding::new(
                name,
                DatatableFilterOp::Datetime,
                DatatableFilterValueKind::DateTime,
            ),
        }
    }

    pub fn text(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Text)
    }

    pub fn text_like(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Text,
            DatatableFilterOp::Like,
            DatatableFilterValueKind::Text,
        )
    }

    pub fn text_search(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Text,
            DatatableFilterOp::LikeAny,
            DatatableFilterValueKind::Text,
        )
    }

    pub fn number(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(name, label, DatatableFilterKind::Number)
    }

    pub fn decimal_min(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Number,
            DatatableFilterOp::Gte,
            DatatableFilterValueKind::Decimal,
        )
    }

    pub fn decimal_max(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Number,
            DatatableFilterOp::Lte,
            DatatableFilterValueKind::Decimal,
        )
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

    pub fn date_from(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Date,
            DatatableFilterOp::DateFrom,
            DatatableFilterValueKind::Date,
        )
    }

    pub fn date_to(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new_with_binding(
            name,
            label,
            DatatableFilterKind::Date,
            DatatableFilterOp::DateTo,
            DatatableFilterValueKind::Date,
        )
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

    pub fn server_field(mut self, field: impl Into<String>) -> Self {
        self.binding.field = field.into();
        self
    }

    pub fn bind(
        mut self,
        field: impl Into<String>,
        op: DatatableFilterOp,
        value_kind: DatatableFilterValueKind,
    ) -> Self {
        self.binding = DatatableFilterBinding::new(field, op, value_kind);
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

    use crate::datatable::{DatatableFilterOp, DatatableFilterValueKind};

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
                "binding": {
                    "field": "status",
                    "op": "eq",
                    "value_kind": "text"
                },
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
                "binding": {
                    "field": "status",
                    "op": "eq",
                    "value_kind": "text"
                },
                "placeholder": "Search status",
                "help": "Filters by status",
                "nullable": false,
                "options": {
                    "items": []
                }
            })
        );
    }

    #[test]
    fn bind_overrides_server_field_operator_and_value_kind() {
        let filter = DatatableFilterField::number("minimum_amount", "Minimum Amount").bind(
            "amount",
            DatatableFilterOp::Gte,
            DatatableFilterValueKind::Decimal,
        );

        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({
                "name": "minimum_amount",
                "kind": "number",
                "label": "Minimum Amount",
                "binding": {
                    "field": "amount",
                    "op": "gte",
                    "value_kind": "decimal"
                },
                "nullable": false,
                "options": {
                    "items": []
                }
            })
        );
    }

    #[test]
    fn semantic_helpers_provide_expected_default_bindings() {
        let text_like = DatatableFilterField::text_like("status", "Status");
        assert_eq!(text_like.binding.field, "status");
        assert_eq!(text_like.binding.op, DatatableFilterOp::Like);
        assert_eq!(text_like.binding.value_kind, DatatableFilterValueKind::Text);

        let text_search = DatatableFilterField::text_search("query", "Search");
        assert_eq!(text_search.binding.field, "query");
        assert_eq!(text_search.binding.op, DatatableFilterOp::LikeAny);
        assert_eq!(
            text_search.binding.value_kind,
            DatatableFilterValueKind::Text
        );

        let date_from = DatatableFilterField::date_from("starts_on", "Starts On");
        assert_eq!(date_from.binding.field, "starts_on");
        assert_eq!(date_from.binding.op, DatatableFilterOp::DateFrom);
        assert_eq!(date_from.binding.value_kind, DatatableFilterValueKind::Date);

        let decimal_min = DatatableFilterField::decimal_min("minimum_total", "Minimum Total");
        assert_eq!(decimal_min.binding.field, "minimum_total");
        assert_eq!(decimal_min.binding.op, DatatableFilterOp::Gte);
        assert_eq!(
            decimal_min.binding.value_kind,
            DatatableFilterValueKind::Decimal
        );
    }

    #[test]
    fn server_field_overrides_only_the_binding_field() {
        let filter = DatatableFilterField::decimal_max("maximum_total", "Maximum Total")
            .server_field("total");

        assert_eq!(filter.name, "maximum_total");
        assert_eq!(filter.binding.field, "total");
        assert_eq!(filter.binding.op, DatatableFilterOp::Lte);
        assert_eq!(filter.binding.value_kind, DatatableFilterValueKind::Decimal);
    }
}
