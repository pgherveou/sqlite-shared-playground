//! Shared purse operations executed on a native thread or a dedicated WASM worker.

mod db;
mod error;
mod operation;
#[cfg(any(target_arch = "wasm32", test))]
mod proof;
mod purse;
#[cfg(any(target_arch = "wasm32", test))]
mod registry;
mod startup;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web_api;

pub use db::Db;
pub use error::Error;
pub use operation::{Access, Operation};
pub use purse::{PurseDao, PurseId, PurseRecord, PurseTag};
pub use startup::initialize_database;
#[cfg(target_arch = "wasm32")]
pub use web_api::serve_database;

#[cfg(target_arch = "wasm32")]
use browser::Backend;
#[cfg(not(target_arch = "wasm32"))]
use native::Backend;

/// Typed operation dispatch independent of connection placement.
#[derive(Clone)]
pub struct Client {
    backend: Backend,
}

impl Client {
    /// Opens a native writer and two readers; private temporary databases use only the writer.
    ///
    /// Runs `initialize` on the writer before readers open. Initialization errors
    /// prevent the client from becoming ready.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open(
        path: impl AsRef<std::path::Path>,
        initialize: fn(&mut Db) -> Result<(), Error>,
    ) -> Result<Self, Error> {
        let path = path.as_ref();
        let readers = if path == std::path::Path::new(":memory:") || path.as_os_str().is_empty() {
            0
        } else {
            2
        };
        Self::open_with_readers(path, readers, initialize).await
    }

    /// Opens one native writer and the requested number of read-only connections.
    ///
    /// Runs `initialize` on the writer before readers open.
    /// Zero readers routes all work to the writer. Multiple connections require
    /// a file-backed WAL database; private memory and temporary databases are rejected.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_with_readers(
        path: impl AsRef<std::path::Path>,
        readers: usize,
        initialize: fn(&mut Db) -> Result<(), Error>,
    ) -> Result<Self, Error> {
        Ok(Self {
            backend: Backend::open(path, readers, initialize).await?,
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

    /// Drains admitted work and releases the database.
    pub async fn close(&self) -> Result<(), Error> {
        self.backend.close().await
    }
}
