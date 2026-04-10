use anyhow::anyhow;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Anyhow(anyhow!(message.into()))
    }

    pub fn other<E>(error: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self::Anyhow(error.into())
    }
}
