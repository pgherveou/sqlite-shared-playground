use futures_channel::mpsc;
use futures_util::StreamExt;
use rusqlite::functions::FunctionFlags;
use wasm_bindgen::{JsCast, prelude::*};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use super::wire::{self, Command, Reply, Request, VERSION};
use crate::{Db, Error, registry::registry};

#[wasm_bindgen(inline_js = "
export function waitForRelease() {
    const request = new XMLHttpRequest();
    request.open('GET', '/query-gate', false);
    request.send();
    if (request.status !== 200) throw new Error('query gate failed');
}
")]
extern "C" {
    #[wasm_bindgen(catch, js_name = waitForRelease)]
    fn wait_for_release() -> Result<(), JsValue>;
}

struct Listener {
    scope: DedicatedWorkerGlobalScope,
    _callback: Closure<dyn FnMut(MessageEvent)>,
}

impl Listener {
    fn attach(sender: mpsc::UnboundedSender<JsValue>) -> Result<Self, JsValue> {
        let scope = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>()?;
        let callback = Closure::new(move |event: MessageEvent| {
            let _ = sender.unbounded_send(event.data());
        });
        scope.set_onmessage(Some(callback.as_ref().unchecked_ref()));
        Ok(Self {
            scope,
            _callback: callback,
        })
    }

    fn reply(&self, id: u64, result: Result<Vec<u8>, Error>) -> Result<(), JsValue> {
        let message = wire::encode(&Reply {
            version: VERSION,
            id,
            result,
        })
        .map_err(js_error)?;
        self.scope.post_message(&message)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.scope.set_onmessage(None);
    }
}

fn js_error(error: impl core::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

/// Opens OPFS and serves registered operations on this database-only worker.
///
/// The entry script must buffer messages while loading WASM, then replay them
/// after invoking this function. Closing acknowledges SQLite and VFS cleanup.
#[wasm_bindgen]
pub fn serve_database(name: String) -> Result<js_sys::Promise, JsValue> {
    let (sender, receiver) = mpsc::unbounded();
    let listener = Listener::attach(sender)?;
    Ok(wasm_bindgen_futures::future_to_promise(async move {
        serve(name, receiver, listener).await?;
        Ok(JsValue::UNDEFINED)
    }))
}

async fn serve(
    name: String,
    mut receiver: mpsc::UnboundedReceiver<JsValue>,
    listener: Listener,
) -> Result<(), JsValue> {
    use sqlite_wasm_vfs::sahpool::{OpfsSAHPoolCfg, install};

    let initialized = async {
        let registry = registry()?;
        let pool = install::<sqlite_wasm_rs::WasmOsCallback>(&OpfsSAHPoolCfg::default(), true)
            .await
            .map_err(|error| Error::Transport(error.to_string()))?;
        let db = match Db::open(name) {
            Ok(db) => db,
            Err(error) => {
                let _ = pool.pause();
                return Err(error);
            }
        };
        let configured = db.read(|connection| {
            connection.create_scalar_function(
                "wait_for_release",
                0,
                FunctionFlags::SQLITE_UTF8,
                |_| {
                    wait_for_release().map_err(|error| {
                        rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                            wire::js_error(error),
                        )))
                    })?;
                    Ok(0_i64)
                },
            )?;
            Ok(())
        });
        if let Err(error) = configured {
            let _ = db.close();
            let _ = pool.pause();
            return Err(error);
        }
        Ok::<_, Error>((db, pool, registry))
    }
    .await;

    let (mut db, pool, registry) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            if let Some(message) = receiver.next().await {
                let request: Request = wire::decode(message).map_err(js_error)?;
                listener.reply(request.id, Err(error))?;
            }
            return Ok(());
        }
    };
    while let Some(message) = receiver.next().await {
        let request: Request = wire::decode(message).map_err(js_error)?;
        if request.version != VERSION {
            listener.reply(
                request.id,
                Err(Error::Protocol(
                    "unsupported worker protocol version".into(),
                )),
            )?;
            continue;
        }
        match request.command {
            Command::Hello => listener.reply(request.id, Ok(Vec::new()))?,
            Command::Call { name, input } => {
                let result = registry.dispatch(&name, &input, &mut db);
                listener.reply(request.id, result)?;
            }
            Command::Close => {
                let closed = db.close();
                let released = pool
                    .pause()
                    .map_err(|error| Error::Transport(error.to_string()));
                listener.reply(request.id, closed.and(released).map(|_| Vec::new()))?;
                return Ok(());
            }
        }
    }
    Ok(())
}
