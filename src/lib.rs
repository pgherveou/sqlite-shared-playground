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
    CreatePurse, CreatePurses, HeldQuery, Operation, PursesTagged, ReadPurse, SqlFailure,
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
    /// Opens a native database on its dedicated thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        Ok(Self {
            backend: Backend::open(path).await?,
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
    pub async fn call<Work: Operation + serde::Serialize>(
        &self,
        work: Work,
    ) -> Result<Work::Output, Error>
    where
        Work::Output: serde::de::DeserializeOwned,
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
