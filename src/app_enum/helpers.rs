/// Convert CamelCase to snake_case.
/// Pending -> pending, ActiveIncome -> active_income, InReview -> in_review
pub fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert CamelCase to title text (insert spaces before uppercase letters).
/// Pending -> Pending, ActiveIncome -> Active Income, InReview -> In Review
pub fn to_title_text(name: &str) -> String {
    let mut result = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            result.push(' ');
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_simple() {
        assert_eq!(to_snake_case("Pending"), "pending");
    }

    #[test]
    fn snake_case_multi_word() {
        assert_eq!(to_snake_case("ActiveIncome"), "active_income");
    }

    #[test]
    fn snake_case_single_char() {
        assert_eq!(to_snake_case("X"), "x");
    }

    #[test]
    fn snake_case_already_lower() {
        assert_eq!(to_snake_case("active"), "active");
    }

    #[test]
    fn title_text_simple() {
        assert_eq!(to_title_text("Pending"), "Pending");
    }

    #[test]
    fn title_text_multi_word() {
        assert_eq!(to_title_text("ActiveIncome"), "Active Income");
    }

    #[test]
    fn title_text_single_char() {
        assert_eq!(to_title_text("X"), "X");
    }
}
