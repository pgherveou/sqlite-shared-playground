//! One native writer and independently executing read-only connections.

use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
};

use futures_channel::oneshot;
use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared, join_all},
};

use crate::{Access, Db, Error, Operation};

type Job = Box<dyn FnOnce(&mut Db) -> bool + Send>;
type Completion = Shared<BoxFuture<'static, Result<(), Error>>>;

enum Message {
    Execute(Job),
    Close(oneshot::Sender<Result<(), Error>>),
}

enum State {
    Open(Connections),
    Closing(Completion),
}

struct Connections {
    writer: mpsc::Sender<Message>,
    readers: Vec<mpsc::Sender<Message>>,
    next_reader: usize,
}

impl Connections {
    fn select(&mut self, access: Access) -> &mpsc::Sender<Message> {
        match access {
            Access::Read if !self.readers.is_empty() => {
                let index = self.next_reader;
                self.next_reader = (index + 1) % self.readers.len();
                &self.readers[index]
            }
            Access::Read | Access::Write => &self.writer,
        }
    }
}

/// Connection owners and admission state shared by cloned clients.
#[derive(Clone)]
pub struct Backend {
    state: Arc<Mutex<State>>,
}

impl Backend {
    /// Initializes the writer before opening any read-only connections.
    pub async fn open(path: impl AsRef<Path>, readers: usize) -> Result<Self, Error> {
        let path = path.as_ref().to_owned();
        if readers > 0 && (path == Path::new(":memory:") || path.as_os_str().is_empty()) {
            return Err(Error::Refused(
                "readers require a file-backed WAL database".into(),
            ));
        }
        let writer_path = path.clone();
        let writer = open_connection("sqlite-writer".into(), move || {
            let db = Db::open(writer_path)?;
            if readers > 0 {
                db.read(|connection| {
                    let mode: String =
                        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
                            row.get(0)
                        })?;
                    if !mode.eq_ignore_ascii_case("wal") {
                        return Err(Error::Refused(
                            "readers require a file-backed WAL database".into(),
                        ));
                    }
                    Ok(())
                })?;
            }
            Ok(db)
        })
        .await?;
        let mut connections = Connections {
            writer,
            readers: Vec::new(),
            next_reader: 0,
        };
        for index in 0..readers {
            let path = path.clone();
            connections.readers.push(
                open_connection(format!("sqlite-reader-{index}"), move || {
                    Db::open_reader(path)
                })
                .await?,
            );
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State::Open(connections))),
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
            let mut state = self
                .state
                .lock()
                .map_err(|_| Error::Transport("connection state poisoned".into()))?;
            match &mut *state {
                State::Open(connections) => connections
                    .select(Command::ACCESS)
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
                State::Open(connections) => {
                    let replies = std::iter::once(&connections.writer)
                        .chain(&connections.readers)
                        .map(|sender| {
                            let (reply, response) = oneshot::channel();
                            let sent = sender.send(Message::Close(reply));
                            async move {
                                sent.map_err(|_| {
                                    Error::OutcomeUnknown("connection stopped before close".into())
                                })?;
                                response.await.map_err(|_| {
                                    Error::OutcomeUnknown("connection stopped while closing".into())
                                })?
                            }
                        })
                        .collect::<Vec<_>>();
                    let completion = async move {
                        join_all(replies)
                            .await
                            .into_iter()
                            .collect::<Result<Vec<_>, _>>()
                            .map(|_| ())
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

async fn open_connection(
    name: String,
    open: impl FnOnce() -> Result<Db, Error> + Send + 'static,
) -> Result<mpsc::Sender<Message>, Error> {
    let (sender, receiver) = mpsc::channel();
    let (ready, startup) = oneshot::channel();
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let db = match open() {
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
    Ok(sender)
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
