use std::collections::HashSet;

/// Sanitize HTML by whitelisting allowed tags and stripping everything else.
///
/// Uses the `ammonia` crate for robust HTML parsing that handles malformed HTML,
/// nested tag attacks (`<scr<script>ipt>`), and browser parsing quirks.
///
/// ```rust
/// use forge::support::sanitize_html;
///
/// let safe = sanitize_html(
///     "<p>Hello <b>world</b></p><script>alert(1)</script>",
///     &["p", "b", "i", "em", "strong"],
/// );
/// assert_eq!(safe, "<p>Hello <b>world</b></p>");
/// ```
pub fn sanitize_html(input: &str, allowed_tags: &[&str]) -> String {
    let tags: HashSet<&str> = allowed_tags.iter().copied().collect();
    ammonia::Builder::default()
        .tags(tags)
        .clean(input)
        .to_string()
}

/// Strip all HTML tags from input, keeping only text content.
pub fn strip_tags(input: &str) -> String {
    ammonia::Builder::default()
        .tags(HashSet::new())
        .clean(input)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_tags() {
        let input = "<p>Hello</p><script>alert(1)</script><p>World</p>";
        let result = sanitize_html(input, &["p"]);
        assert_eq!(result, "<p>Hello</p><p>World</p>");
    }

    #[test]
    fn keeps_allowed_tags() {
        let input = "<b>bold</b> and <i>italic</i> and <u>underline</u>";
        let result = sanitize_html(input, &["b", "i"]);
        assert_eq!(result, "<b>bold</b> and <i>italic</i> and underline");
    }

    #[test]
    fn strips_all_tags_when_empty_allowlist() {
        let input = "<b>bold</b> text <i>here</i>";
        assert_eq!(strip_tags(input), "bold text here");
    }

    #[test]
    fn strips_event_handler_attributes() {
        let input = r#"<a href="https://example.com" onclick="alert(1)">link</a>"#;
        let result = sanitize_html(input, &["a"]);
        assert!(result.contains("https://example.com"));
        assert!(!result.contains("onclick"));
    }

    #[test]
    fn strips_javascript_uri() {
        let input = r#"<a href="javascript:alert(1)">link</a>"#;
        let result = sanitize_html(input, &["a"]);
        assert!(!result.contains("javascript"));
    }

    #[test]
    fn handles_nested_tag_attack() {
        let input = "<scr<script>ipt>alert(1)</scr</script>ipt>";
        let result = sanitize_html(input, &["p", "b"]);
        assert!(!result.contains("<script"));
        assert!(!result.contains("</script"));
    }

    #[test]
    fn handles_empty_input() {
        assert_eq!(sanitize_html("", &["p"]), "");
        assert_eq!(strip_tags(""), "");
    }

    #[test]
    fn handles_no_html() {
        assert_eq!(sanitize_html("plain text", &["p"]), "plain text");
    }

    #[test]
    fn case_insensitive_tag_matching() {
        let input = "<B>bold</B>";
        let result = sanitize_html(input, &["b"]);
        assert!(result.contains("bold"));
    }
}
