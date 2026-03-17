use std::error::Error;
use std::fmt::{Display, Formatter};

/// Errors returned by `pdcore` constructors and validators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// A required field is empty or only whitespace.
    EmptyField(&'static str),
    /// No collector metadata was provided.
    NoCollectors,
    /// Collector ordering value is duplicated.
    DuplicateCollectorOrder {
        /// Duplicated order index.
        order: usize,
    },
    /// Collector key is duplicated.
    DuplicateCollectorKey {
        /// Duplicated collector key.
        key: String,
    },
    /// Metric values exceeded fixed capacity.
    TooManyMetricValues {
        /// Actual number of values provided.
        got: usize,
        /// Maximum accepted number of values.
        max: usize,
    },
    /// Control command string could not be parsed.
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
