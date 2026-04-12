#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StoredFile {
    pub disk: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub url: Option<String>,
}
