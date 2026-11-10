use wasm_bindgen::prelude::*;
use web_sys::{Worker, WorkerOptions, WorkerType};

use crate::{
    Client, PurseDao, PurseId, PurseRecord, PurseTag, initialize_database,
    proof::{HeldQuery, SqlFailure},
    startup,
};

fn message(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn purse_value(purse: PurseRecord) -> Result<JsValue, JsValue> {
    let value = js_sys::Object::new();
    js_sys::Reflect::set(&value, &"id".into(), &purse.id.0.into())?;
    js_sys::Reflect::set(&value, &"name".into(), &purse.name.into())?;
    let tag = js_sys::Array::from_iter(purse.tag.0.into_iter().map(JsValue::from));
    js_sys::Reflect::set(&value, &"tag".into(), &tag)?;
    Ok(value.into())
}

/// A Rust caller whose database lives in a different worker and WASM instance.
#[wasm_bindgen]
pub struct BrowserClient {
    client: Client,
    purses: PurseDao,
}

#[wasm_bindgen]
impl BrowserClient {
    /// Starts a database worker and waits for persistent storage readiness.
    pub async fn open(url: String) -> Result<BrowserClient, JsValue> {
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&url, &options)?;
        let client = Client::connect(worker).await.map_err(message)?;
        let purses = PurseDao::new(client.clone());
        Ok(Self { client, purses })
    }

    /// Creates a purse using the shared Rust operation.
    pub async fn create_purse(&self, name: String, tag: Vec<u8>) -> Result<Option<u32>, JsValue> {
        self.purses
            .create(name, PurseTag(tag))
            .await
            .map(|id| id.map(|id| id.0))
            .map_err(message)
    }

    /// Looks up a purse through the registered worker handler.
    pub async fn purse(&self, id: u32) -> Result<JsValue, JsValue> {
        match self.purses.get(PurseId(id)).await.map_err(message)? {
            Some(purse) => purse_value(purse),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Lists matching purses through the same facade used by the native example.
    pub async fn purses_tagged(&self, tag: Vec<u8>) -> Result<js_sys::Array, JsValue> {
        self.purses
            .tagged(PurseTag(tag))
            .await
            .map_err(message)?
            .into_iter()
            .map(purse_value)
            .collect()
    }

    /// Creates a complete batch atomically.
    pub async fn create_purses(
        &self,
        names: Vec<String>,
        tag: Vec<u8>,
    ) -> Result<js_sys::Array, JsValue> {
        let ids = self
            .purses
            .create_batch(names, PurseTag(tag))
            .await
            .map_err(message)?;
        Ok(js_sys::Array::from_iter(
            ids.into_iter().map(|id| JsValue::from(id.0)),
        ))
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

/// Opens the playground's domains and serves them on a dedicated database worker.
///
/// The entry script must buffer messages while loading WASM and replay them
/// after calling this function.
#[wasm_bindgen]
pub fn serve_database(name: String) -> Result<js_sys::Promise, JsValue> {
    crate::browser::serve_database(
        name,
        startup::registry().map_err(message)?,
        initialize_database,
    )
}
