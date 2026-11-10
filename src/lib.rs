//! Shared purse operations executed on a native thread or a dedicated WASM worker.

mod db;
mod error;
mod model;
mod operation;
#[cfg(any(target_arch = "wasm32", test))]
mod registry;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web_api;

#[cfg(target_arch = "wasm32")]
pub use browser::serve_database;
pub use db::Db;
pub use error::Error;
pub use model::{PurseId, PurseRecord, PurseTag};
pub use operation::{
    Access, CreatePurse, CreatePurses, HeldQuery, Operation, PursesTagged, ReadPurse, SqlFailure,
};

#[cfg(target_arch = "wasm32")]
use browser::Backend;
#[cfg(not(target_arch = "wasm32"))]
use native::Backend;

/// Typed domain methods independent of connection placement.
#[derive(Clone)]
pub struct Client {
    backend: Backend,
}

impl Client {
    /// Opens a native writer and two readers; private temporary databases use only the writer.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let readers = if path == std::path::Path::new(":memory:") || path.as_os_str().is_empty() {
            0
        } else {
            2
        };
        Self::open_with_readers(path, readers).await
    }

    /// Opens one native writer and the requested number of read-only connections.
    ///
    /// Zero readers routes all work to the writer. Multiple connections require
    /// a file-backed WAL database; private memory and temporary databases are rejected.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_with_readers(
        path: impl AsRef<std::path::Path>,
        readers: usize,
    ) -> Result<Self, Error> {
        Ok(Self {
            backend: Backend::open(path, readers).await?,
        })
    }

    /// Connects to a separate browser database worker.
    #[cfg(target_arch = "wasm32")]
    pub async fn connect(worker: web_sys::Worker) -> Result<Self, Error> {
        Ok(Self {
            backend: Backend::connect(worker).await?,
        })
    }

    /// Executes typed work without serialization on native targets.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn call<Work: Operation + Send + 'static>(
        &self,
        work: Work,
    ) -> Result<Work::Output, Error>
    where
        Work::Output: Send + 'static,
    {
        self.backend.call(work).await
    }

    /// Sends a registered operation to the database worker.
    #[cfg(target_arch = "wasm32")]
    pub async fn call<Work: Operation + parity_scale_codec::Encode>(
        &self,
        work: Work,
    ) -> Result<Work::Output, Error>
    where
        Work::Output: parity_scale_codec::Decode,
    {
        self.backend.call(work).await
    }

    /// Creates a purse under the next free identifier.
    pub async fn create_purse(
        &self,
        name: String,
        tag: PurseTag,
    ) -> Result<Option<PurseId>, Error> {
        self.call(CreatePurse { name, tag }).await
    }

    /// Reads one purse, returning `None` when absent.
    pub async fn purse(&self, id: PurseId) -> Result<Option<PurseRecord>, Error> {
        self.call(ReadPurse { id }).await
    }

    /// Lists matching purses in identifier order.
    pub async fn purses_tagged(&self, tag: PurseTag) -> Result<Vec<PurseRecord>, Error> {
        self.call(PursesTagged { tag }).await
    }

    /// Commits an entire batch or rolls it back on refusal.
    pub async fn create_purses(
        &self,
        names: Vec<String>,
        tag: PurseTag,
    ) -> Result<Vec<PurseId>, Error> {
        self.call(CreatePurses { names, tag }).await
    }

    /// Drains admitted work and releases the database.
    pub async fn close(&self) -> Result<(), Error> {
        self.backend.close().await
    }
}
