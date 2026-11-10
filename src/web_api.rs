use wasm_bindgen::prelude::*;
use web_sys::{Worker, WorkerOptions, WorkerType};

use crate::{Client, HeldQuery, PurseId, PurseTag, SqlFailure};

fn message(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn value(output: impl serde::Serialize) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&output).map_err(message)
}

/// A Rust caller whose database lives in a different worker and WASM instance.
#[wasm_bindgen]
pub struct BrowserClient {
    client: Client,
}

#[wasm_bindgen]
impl BrowserClient {
    /// Starts a database worker and waits for persistent storage readiness.
    pub async fn open(url: String) -> Result<BrowserClient, JsValue> {
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&url, &options)?;
        let client = Client::connect(worker).await.map_err(message)?;
        Ok(Self { client })
    }

    /// Creates a purse using the shared Rust operation.
    pub async fn create_purse(&self, name: String, tag: Vec<u8>) -> Result<JsValue, JsValue> {
        value(
            self.client
                .create_purse(name, PurseTag(tag))
                .await
                .map_err(message)?,
        )
    }

    /// Looks up a purse through the registered worker handler.
    pub async fn purse(&self, id: u32) -> Result<JsValue, JsValue> {
        value(self.client.purse(PurseId(id)).await.map_err(message)?)
    }

    /// Lists matching purses through the same facade used by the native example.
    pub async fn purses_tagged(&self, tag: Vec<u8>) -> Result<JsValue, JsValue> {
        value(
            self.client
                .purses_tagged(PurseTag(tag))
                .await
                .map_err(message)?,
        )
    }

    /// Creates a complete batch atomically.
    pub async fn create_purses(&self, names: JsValue, tag: Vec<u8>) -> Result<JsValue, JsValue> {
        let names = serde_wasm_bindgen::from_value(names).map_err(message)?;
        value(
            self.client
                .create_purses(names, PurseTag(tag))
                .await
                .map_err(message)?,
        )
    }

    /// Demonstrates propagation of an SQLite error without closing the connection.
    pub async fn sql_failure(&self) -> Result<(), JsValue> {
        self.client.call(SqlFailure).await.map_err(message)
    }

    /// Holds an SQLite statement at the browser test server's release gate.
    pub async fn held_query(&self) -> Result<String, JsValue> {
        self.client
            .call(HeldQuery)
            .await
            .map(|number| number.to_string())
            .map_err(message)
    }

    /// Demonstrates that an unregistered operation is rejected.
    pub async fn unknown_operation(&self) -> Result<(), JsValue> {
        self.client
            .backend
            .request("unregistered.v1", Vec::new())
            .await
            .map(|_| ())
            .map_err(message)
    }

    /// Closes admitted operations and releases the persistent database.
    pub async fn close(&self) -> Result<(), JsValue> {
        self.client.close().await.map_err(message)
    }

    /// Stops the worker and reports uncertain outcomes to pending callers.
    pub fn terminate(&self) {
        self.client.backend.terminate();
    }
}

/// Proves that Rust in the caller worker can run while the database worker is busy.
#[wasm_bindgen]
pub fn ping() -> String {
    "Rust caller responsive".into()
}
