use serde::{Deserialize, Serialize};

/// An email address with an optional display name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmailAddress {
    address: String,
    name: Option<String>,
}

impl EmailAddress {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            name: None,
        }
    }

    pub fn with_name(address: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            name: Some(name.into()),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl From<&str> for EmailAddress {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EmailAddress {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl std::fmt::Display for EmailAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{} <{}>", name, self.address),
            None => write!(f, "{}", self.address),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn email_address_new_creates_without_name() {
        let email = EmailAddress::new("test@example.com");
        assert_eq!(email.address(), "test@example.com");
        assert_eq!(email.name(), None);
    }

    #[test]
    fn email_address_with_name_sets_both() {
        let email = EmailAddress::with_name("test@example.com", "Test User");
        assert_eq!(email.address(), "test@example.com");
        assert_eq!(email.name(), Some("Test User"));
    }

    #[test]
    fn email_address_display_without_name() {
        let email = EmailAddress::new("test@example.com");
        assert_eq!(email.to_string(), "test@example.com");
    }

    #[test]
    fn email_address_display_with_name() {
        let email = EmailAddress::with_name("test@example.com", "Test User");
        assert_eq!(email.to_string(), "Test User <test@example.com>");
    }

    #[test]
    fn email_address_from_str() {
        let email: EmailAddress = "test@example.com".into();
        assert_eq!(email.address(), "test@example.com");
        assert_eq!(email.name(), None);
    }

    #[test]
    fn email_address_from_string() {
        let email: EmailAddress = "test@example.com".to_string().into();
        assert_eq!(email.address(), "test@example.com");
        assert_eq!(email.name(), None);
    }

    #[test]
    fn email_address_serialization_roundtrip() {
        let original = EmailAddress::with_name("test@example.com", "Test User");
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: EmailAddress = serde_json::from_str(&serialized).unwrap();
        assert_eq!(original, deserialized);
    }
}
