//! Connection ownership and ordered operation execution on a native thread.

use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
};

use futures_channel::oneshot;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};

use crate::{Db, Error, Operation};

type Job = Box<dyn FnOnce(&mut Db) -> bool + Send>;
type Completion = Shared<BoxFuture<'static, Result<(), Error>>>;

enum Message {
    Execute(Job),
    Close(oneshot::Sender<Result<(), Error>>),
}

enum State {
    Open(mpsc::Sender<Message>),
    Closing(Completion),
}

/// A connection thread shared by cloned clients.
#[derive(Clone)]
pub struct Backend {
    state: Arc<Mutex<State>>,
}

impl Backend {
    /// Opens the connection on its owner thread before admitting operations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_owned();
        let (sender, receiver) = mpsc::channel();
        let (ready, startup) = oneshot::channel();
        std::thread::Builder::new()
            .name("sqlite-owner".into())
            .spawn(move || {
                let db = match Db::open(path) {
                    Ok(db) => db,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(())).is_ok() {
                    serve(db, receiver);
                }
            })
            .map_err(|error| Error::Transport(error.to_string()))?;
        startup
            .await
            .map_err(|_| Error::Transport("connection startup stopped".into()))??;
        Ok(Self {
            state: Arc::new(Mutex::new(State::Open(sender))),
        })
    }

    /// Admits one operation; dropping its reply does not cancel accepted work.
    pub async fn call<Command>(&self, operation: Command) -> Result<Command::Output, Error>
    where
        Command: Operation + Send + 'static,
        Command::Output: Send + 'static,
    {
        let (reply, response) = oneshot::channel();
        let job =
            move |db: &mut Db| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                operation.execute(db)
            })) {
                Ok(result) => {
                    let _ = reply.send(result);
                    true
                }
                Err(_) => {
                    let _ = reply.send(Err(Error::OutcomeUnknown(
                        "operation panicked; connection stopped".into(),
                    )));
                    false
                }
            };
        {
            let state = self
                .state
                .lock()
                .map_err(|_| Error::Transport("connection state poisoned".into()))?;
            match &*state {
                State::Open(sender) => sender
                    .send(Message::Execute(Box::new(job)))
                    .map_err(|_| Error::Closed)?,
                State::Closing(_) => return Err(Error::Closed),
            }
        }
        response
            .await
            .map_err(|_| Error::OutcomeUnknown("connection stopped before replying".into()))?
    }

    /// Stops admission, drains accepted work and shares the close result.
    pub async fn close(&self) -> Result<(), Error> {
        let completion = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Error::Transport("connection state poisoned".into()))?;
            match &*state {
                State::Closing(completion) => completion.clone(),
                State::Open(sender) => {
                    let (reply, response) = oneshot::channel();
                    let sent = sender.send(Message::Close(reply));
                    let completion = async move {
                        sent.map_err(|_| {
                            Error::OutcomeUnknown("connection stopped before close".into())
                        })?;
                        response.await.map_err(|_| {
                            Error::OutcomeUnknown("connection stopped while closing".into())
                        })?
                    }
                    .boxed()
                    .shared();
                    *state = State::Closing(completion.clone());
                    completion
                }
            }
        };
        completion.await
    }
}

fn serve(mut db: Db, receiver: mpsc::Receiver<Message>) {
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Execute(job) => {
                if !job(&mut db) {
                    return;
                }
            }
            Message::Close(reply) => {
                let _ = reply.send(db.close());
                return;
            }
        }
    }
}
