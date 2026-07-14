//! Engine error type. Data sources report their own errors as `eyre::Report`
//! (see [`crate::DocSource`]); the engine only surfaces index/IO/manifest
//! failures here.

pub type Result<T> = std::result::Result<T, KbError>;

#[derive(Debug, thiserror::Error)]
pub enum KbError {
    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("index directory error: {0}")]
    OpenDir(#[from] tantivy::directory::error::OpenDirectoryError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("manifest (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
