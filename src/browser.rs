//! Typed operations sent to a dedicated database worker.
//!
//! ```text
//! Caller WASM                 Database worker WASM
//! Backend -- arguments --> Registry --> Db --> DAO
//!         <-- result -----
//! ```

mod caller;
mod server;
mod wire;

pub use caller::Backend;
pub use server::serve_database;
