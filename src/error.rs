use serde::{Deserialize, Serialize};

/// Failures crossing the operation boundary, including uncertain worker outcomes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum Error {
    /// SQLite rejected the operation.
    #[error("SQLite: {0}")]
    Sqlite(String),
    /// An application rule refused the transaction.
    #[error("refused: {0}")]
    Refused(String),
    /// The executor no longer accepts work.
    #[error("database closed")]
    Closed,
    /// A request could not be transported.
    #[error("transport: {0}")]
    Transport(String),
    /// A request or reply does not match the registered protocol.
    #[error("protocol: {0}")]
    Protocol(String),
    /// Work may have committed before its executor disappeared.
    #[error("outcome unknown: {0}")]
    OutcomeUnknown(String),
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error.to_string())
    }
}
