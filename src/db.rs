use std::path::Path;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::Error;

/// A connection owned exclusively by its executor.
pub struct Db {
    connection: Connection,
}

impl Db {
    /// Opens persistent storage with foreign-key enforcement enabled.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::initialize(Connection::open(path)?)
    }

    /// Opens a native reader after the writer has initialized the schema.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_reader(path: impl AsRef<Path>) -> Result<Self, Error> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { connection })
    }

    /// Opens an isolated database for tests.
    pub fn memory() -> Result<Self, Error> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, Error> {
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { connection })
    }

    /// Executes a synchronous query on the owned connection.
    pub fn read<Output>(
        &self,
        query: impl FnOnce(&Connection) -> Result<Output, Error>,
    ) -> Result<Output, Error> {
        query(&self.connection)
    }

    /// Commits the complete operation, rolling back on any error or refusal.
    pub fn write<Output>(
        &mut self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<Output, Error>,
    ) -> Result<Output, Error> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let output = operation(&transaction)?;
        transaction.commit()?;
        Ok(output)
    }

    /// Releases the connection before its backing storage is shut down.
    pub fn close(self) -> Result<(), Error> {
        self.connection.close().map_err(|(_, error)| error.into())
    }
}
