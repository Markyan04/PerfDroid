use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    EmptyField(&'static str),
    NoCollectors,
    DuplicateCollectorOrder { order: usize },
    DuplicateCollectorKey { key: String },
    TooManyMetricValues { got: usize, max: usize },
    InvalidControlCommand(String),
}

impl Display for CoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "field `{field}` cannot be empty"),
            Self::NoCollectors => write!(f, "at least one collector is required"),
            Self::DuplicateCollectorOrder { order } => {
                write!(f, "duplicate collector order `{order}`")
            }
            Self::DuplicateCollectorKey { key } => {
                write!(f, "duplicate collector key `{key}`")
            }
            Self::TooManyMetricValues { got, max } => {
                write!(f, "too many metric values: got {got}, max {max}")
            }
            Self::InvalidControlCommand(command) => {
                write!(f, "invalid control command `{command}`")
            }
        }
    }
}

impl Error for CoreError {}
