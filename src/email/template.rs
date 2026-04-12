use std::path::PathBuf;

use crate::foundation::{Error, Result};

/// Simple email template renderer using `{{variable}}` replacement.
///
/// Templates are loaded from the filesystem at `templates/emails/` by default.
/// Each template can have `.html` and `.txt` variants.
///
/// ```ignore
/// let renderer = TemplateRenderer::new("templates/emails");
/// let (html, text) = renderer.render("welcome", &json!({
///     "name": "Alice",
///     "app_name": "MyApp",
/// }))?;
/// ```
pub struct TemplateRenderer {
    base_path: PathBuf,
}

impl TemplateRenderer {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Render a template by name with the given variables.
    ///
    /// Returns `(Option<html>, Option<text>)` — at least one will be `Some` if the
    /// template exists.
    pub fn render(
        &self,
        template_name: &str,
        variables: &serde_json::Value,
    ) -> Result<RenderedTemplate> {
        let html_path = self.base_path.join(format!("{template_name}.html"));
        let text_path = self.base_path.join(format!("{template_name}.txt"));

        let html = match std::fs::read_to_string(&html_path) {
            Ok(content) => Some(replace_variables(&content, variables)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(Error::message(format!(
                    "failed to read email template '{}': {e}",
                    html_path.display()
                )))
            }
        };

        let text = match std::fs::read_to_string(&text_path) {
            Ok(content) => Some(replace_variables(&content, variables)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(Error::message(format!(
                    "failed to read email template '{}': {e}",
                    text_path.display()
                )))
            }
        };

        if html.is_none() && text.is_none() {
            return Err(Error::message(format!(
                "email template '{template_name}' not found (checked {}.html and {}.txt in {})",
                template_name,
                template_name,
                self.base_path.display()
            )));
        }

        Ok(RenderedTemplate { html, text })
    }

    /// Check if a template exists (either .html or .txt variant).
    pub fn exists(&self, template_name: &str) -> bool {
        let html_path = self.base_path.join(format!("{template_name}.html"));
        let text_path = self.base_path.join(format!("{template_name}.txt"));
        html_path.exists() || text_path.exists()
    }
}

/// Result of rendering an email template.
pub struct RenderedTemplate {
    pub html: Option<String>,
    pub text: Option<String>,
}

/// Replace `{{key}}` placeholders in content with values from the JSON variables.
///
/// Supports nested access via dot notation: `{{user.name}}`.
/// Unmatched placeholders are left as-is.
fn replace_variables(content: &str, variables: &serde_json::Value) -> String {
    let mut result = content.to_string();
    let mut search_from = 0;

    // Find all {{...}} patterns and replace them
    loop {
        let start = match result[search_from..].find("{{") {
            Some(pos) => search_from + pos,
            None => break,
        };
        let end = match result[start..].find("}}") {
            Some(pos) => start + pos + 2,
            None => break,
        };

        let key = result[start + 2..end - 2].trim();
        let replacement = resolve_json_path(variables, key);
        let replacement_len = replacement.len();
        result.replace_range(start..end, &replacement);
        // Advance past the replacement to avoid re-processing unmatched placeholders
        search_from = start + replacement_len;
    }

    result
}

/// Resolve a dot-notation path in a JSON value.
fn resolve_json_path(value: &serde_json::Value, path: &str) -> String {
    let mut current = value;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(v) => current = v,
            None => return format!("{{{{{path}}}}}"), // leave unmatched as-is
        }
    }
    match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replace_simple_variables() {
        let content = "Hello {{name}}, welcome to {{app}}!";
        let vars = json!({"name": "Alice", "app": "MyApp"});
        let result = replace_variables(content, &vars);
        assert_eq!(result, "Hello Alice, welcome to MyApp!");
    }

    #[test]
    fn replace_nested_variables() {
        let content = "Hello {{user.name}}!";
        let vars = json!({"user": {"name": "Bob"}});
        let result = replace_variables(content, &vars);
        assert_eq!(result, "Hello Bob!");
    }

    #[test]
    fn unmatched_variables_preserved() {
        let content = "Hello {{name}}, your {{unknown}} is here.";
        let vars = json!({"name": "Alice"});
        let result = replace_variables(content, &vars);
        assert_eq!(result, "Hello Alice, your {{unknown}} is here.");
    }

    #[test]
    fn whitespace_in_variable_names() {
        let content = "Hello {{ name }}!";
        let vars = json!({"name": "Alice"});
        let result = replace_variables(content, &vars);
        assert_eq!(result, "Hello Alice!");
    }
}
