use thiserror::Error;

#[derive(Debug, Error)]
pub enum SeismicError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Invalid parameter '{param}': {message}")]
    InvalidParameter { param: String, message: String },

    #[error("Unknown tool or header: {0}")]
    NotFound(String),

    #[error("Dimension mismatch: {0}")]
    DimensionMismatch(String),

    #[error("Numerical error: {0}")]
    Numerical(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Operation cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, SeismicError>;
