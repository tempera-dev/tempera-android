use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum AndroidError {
    InvalidInput(String),
    Io(std::io::Error),
    Json(serde_json::Error),
    Backend(String),
    StaleState { expected: u64, actual: u64 },
    Unsupported(String),
}

impl Display for AndroidError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::Backend(message) | Self::Unsupported(message) => {
                write!(f, "{message}")
            }
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::StaleState { expected, actual } => write!(
                f,
                "Android UI changed before the planned action could execute (expected revision {expected}, current revision {actual})"
            ),
        }
    }
}

impl std::error::Error for AndroidError {}

impl From<std::io::Error> for AndroidError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for AndroidError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub type Result<T> = std::result::Result<T, AndroidError>;
