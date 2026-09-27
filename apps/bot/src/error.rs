#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{code}")]
    External {
        code: String,
        retry_after: u64,
        permanent: bool,
    },
    #[error("{0}")]
    Config(&'static str),
    #[error("database_error")]
    Database(#[from] rusqlite::Error),
    #[error("invalid_json")]
    Json(#[from] serde_json::Error),
    #[error("filesystem_error")]
    Io(#[from] std::io::Error),
    #[error("invalid_verification_proof")]
    Proof(#[from] verification_protocol::ProtocolError),
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn external(code: impl Into<String>, permanent: bool) -> Self {
        Self::External {
            code: code.into(),
            retry_after: 0,
            permanent,
        }
    }
    pub fn permanent(&self) -> bool {
        matches!(
            self,
            Self::External {
                permanent: true,
                ..
            } | Self::Config(_)
                | Self::Proof(_)
        )
    }
    pub fn retry_after(&self) -> u64 {
        if let Self::External { retry_after, .. } = self {
            *retry_after
        } else {
            0
        }
    }
}
