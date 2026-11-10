use crate::{Db, Error};

/// Selects the native connection that owns an operation.
pub enum Access {
    /// Runs on a read-only connection when readers are configured.
    Read,
    /// Runs on the single writer, including operations that both read and write.
    Write,
}

/// A complete synchronous database operation, with a stable wire identity.
pub trait Operation {
    /// Versioned name shared by caller and worker.
    const NAME: &'static str;
    /// Defaults to the writer; read operations must not change the database.
    const ACCESS: Access = Access::Write;
    /// Owned result returned after the operation completes.
    type Output;
    /// Executes beside SQLite; handlers must finish transactions before returning.
    fn execute(self, db: &mut Db) -> Result<Self::Output, Error>;
}
