use core::cell::{Cell, RefCell};
use std::{collections::HashMap, rc::Rc};

use futures_channel::oneshot;
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{ErrorEvent, Event, MessageEvent, Worker};

use super::wire::{self, Command, Reply, Request, VERSION};
use crate::{Error, Operation};

type Response = Result<Vec<u8>, Error>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Connecting,
    Open,
    Closing,
    Closed,
}

struct State {
    worker: Worker,
    phase: Cell<Phase>,
    next_id: Cell<u64>,
    pending: RefCell<HashMap<u64, oneshot::Sender<Response>>>,
    closing_id: Cell<Option<u64>>,
    close_waiters: RefCell<Vec<oneshot::Sender<Result<(), Error>>>>,
    close_result: RefCell<Option<Result<(), Error>>>,
}

impl State {
    fn fail(&self, error: Error) {
        self.phase.set(Phase::Closed);
        self.worker.terminate();
        for (_, sender) in self.pending.take() {
            let _ = sender.send(Err(error.clone()));
        }
        self.complete_close(Err(error));
    }

    fn complete_close(&self, result: Result<(), Error>) {
        *self.close_result.borrow_mut() = Some(result.clone());
        for waiter in self.close_waiters.take() {
            let _ = waiter.send(result.clone());
        }
    }

    fn receive(&self, reply: Reply) {
        if reply.version != VERSION {
            self.fail(Error::OutcomeUnknown(
                "worker protocol version changed".into(),
            ));
            return;
        }
        if self.closing_id.get() == Some(reply.id) {
            if !self.pending.borrow().is_empty() {
                self.fail(Error::OutcomeUnknown(
                    "worker closed before replying to earlier requests".into(),
                ));
                return;
            }
            self.phase.set(Phase::Closed);
            self.worker.terminate();
            self.complete_close(reply.result.map(|_| ()));
        } else {
            let sender = self.pending.borrow_mut().remove(&reply.id);
            if let Some(sender) = sender {
                let _ = sender.send(reply.result);
            } else {
                self.fail(Error::OutcomeUnknown(
                    "unexpected worker response id".into(),
                ));
            }
        }
    }

    fn allocate_id(&self) -> Result<u64, Error> {
        let id = self.next_id.get();
        self.next_id.set(
            id.checked_add(1)
                .ok_or_else(|| Error::Protocol("request ids exhausted".into()))?,
        );
        Ok(id)
    }

    fn post(&self, id: u64, command: Command) -> Result<(), Error> {
        let message = wire::encode(&Request {
            version: VERSION,
            id,
            command,
        })?;
        self.worker
            .post_message(&message)
            .map_err(|error| Error::Transport(wire::js_error(error)))
    }
}

struct Connection {
    state: Rc<State>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(Event)>,
    _message_error: Closure<dyn FnMut(Event)>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.state.worker.set_onmessage(None);
        self.state.worker.set_onerror(None);
        self.state.worker.set_onmessageerror(None);
        if self.state.phase.get() != Phase::Closed {
            self.state
                .fail(Error::OutcomeUnknown("database client dropped".into()));
        }
    }
}

/// A cloneable connection to one independent database worker.
///
/// Dropping a call does not retract an operation already posted to the worker.
/// Transport failures never automatically retry writes.
#[derive(Clone)]
pub struct Backend {
    connection: Rc<Connection>,
}

impl Backend {
    /// Waits until the worker has opened storage and accepted this protocol.
    pub async fn connect(worker: Worker) -> Result<Self, Error> {
        let state = Rc::new(State {
            worker,
            phase: Cell::new(Phase::Connecting),
            next_id: Cell::new(1),
            pending: RefCell::new(HashMap::new()),
            closing_id: Cell::new(None),
            close_waiters: RefCell::new(Vec::new()),
            close_result: RefCell::new(None),
        });
        let weak = Rc::downgrade(&state);
        let message = Closure::new(move |event: MessageEvent| {
            if let Some(state) = weak.upgrade() {
                match wire::decode::<Reply>(event.data()) {
                    Ok(reply) => state.receive(reply),
                    Err(error) => state.fail(Error::OutcomeUnknown(error.to_string())),
                }
            }
        });
        let weak = Rc::downgrade(&state);
        let error = Closure::new(move |event: Event| {
            event.prevent_default();
            if let Some(state) = weak.upgrade() {
                let message = event
                    .dyn_ref::<ErrorEvent>()
                    .map(ErrorEvent::message)
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "worker script could not be loaded".into());
                state.fail(Error::OutcomeUnknown(format!(
                    "database worker failed: {message}"
                )));
            }
        });
        let weak = Rc::downgrade(&state);
        let message_error = Closure::new(move |_: Event| {
            if let Some(state) = weak.upgrade() {
                state.fail(Error::OutcomeUnknown(
                    "database reply could not be received".into(),
                ));
            }
        });
        state
            .worker
            .set_onmessage(Some(message.as_ref().unchecked_ref()));
        state
            .worker
            .set_onerror(Some(error.as_ref().unchecked_ref()));
        state
            .worker
            .set_onmessageerror(Some(message_error.as_ref().unchecked_ref()));
        let backend = Self {
            connection: Rc::new(Connection {
                state,
                _message: message,
                _error: error,
                _message_error: message_error,
            }),
        };
        backend.exchange(0, Command::Hello).await?;
        backend.connection.state.phase.set(Phase::Open);
        Ok(backend)
    }

    /// Sends a typed operation and decodes only its final result.
    pub async fn call<OperationType>(
        &self,
        operation: OperationType,
    ) -> Result<OperationType::Output, Error>
    where
        OperationType: Operation + Serialize,
        OperationType::Output: DeserializeOwned,
    {
        let input = postcard::to_allocvec(&operation)
            .map_err(|error| Error::Protocol(error.to_string()))?;
        let output = self.request(OperationType::NAME, input).await?;
        postcard::from_bytes(&output).map_err(|error| {
            Error::OutcomeUnknown(format!("operation result could not be decoded: {error}"))
        })
    }

    /// Dispatches an encoded operation, including protocol probes in this playground.
    pub async fn request(&self, name: &str, payload: Vec<u8>) -> Response {
        let state = &self.connection.state;
        if state.phase.get() != Phase::Open {
            return Err(Error::Closed);
        }
        let id = state.allocate_id()?;
        self.exchange(
            id,
            Command::Call {
                name: name.to_owned(),
                input: payload,
            },
        )
        .await
    }

    async fn exchange(&self, id: u64, command: Command) -> Response {
        let state = &self.connection.state;
        let (sender, receiver) = oneshot::channel();
        state.pending.borrow_mut().insert(id, sender);
        if let Err(error) = state.post(id, command) {
            state.pending.borrow_mut().remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| Error::OutcomeUnknown("database reply channel closed".into()))?
    }

    /// Drains posted operations, closes SQLite, and releases persistent storage.
    pub async fn close(&self) -> Result<(), Error> {
        let state = &self.connection.state;
        if let Some(result) = state.close_result.borrow().clone() {
            return result;
        }
        let (sender, receiver) = oneshot::channel();
        state.close_waiters.borrow_mut().push(sender);
        if state.phase.get() == Phase::Open {
            state.phase.set(Phase::Closing);
            let result = state.allocate_id().and_then(|id| {
                state.closing_id.set(Some(id));
                state.post(id, Command::Close)
            });
            if let Err(error) = result {
                state.fail(Error::OutcomeUnknown(format!(
                    "database close could not be sent: {error}"
                )));
            }
        }
        receiver
            .await
            .map_err(|_| Error::OutcomeUnknown("database close reply lost".into()))?
    }

    /// Terminates immediately; outstanding operations may already have committed.
    pub fn terminate(&self) {
        if self.connection.state.phase.get() != Phase::Closed {
            self.connection
                .state
                .fail(Error::OutcomeUnknown("database worker terminated".into()));
        }
    }
}
