#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc;

use futures_channel::oneshot;
use futures_util::{FutureExt, future::join_all};
use sqlite_shared_playground::{
    Access, Client, Db, Error, Operation, PurseId, PurseRecord, PurseTag,
};

fn tag() -> PurseTag {
    PurseTag(b"native-proof".to_vec())
}

#[tokio::test]
async fn concurrent_creates_keep_each_name_and_allocate_distinct_ids() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open(directory.path().join("purses.sqlite3"))
        .await
        .unwrap();
    let names: Vec<_> = (0..32).map(|index| format!("purse-{index}")).collect();
    let created = join_all(
        names
            .iter()
            .map(|name| client.create_purse(name.clone(), tag())),
    )
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let expected: Vec<_> = names
        .into_iter()
        .zip(created)
        .map(|(name, id)| PurseRecord {
            id: id.unwrap(),
            name,
            tag: tag(),
        })
        .collect();
    assert_eq!(client.purses_tagged(tag()).await.unwrap(), expected);
    let ids: Vec<_> = expected.iter().map(|record| record.id).collect();
    assert_eq!(ids, (0..32).map(PurseId).collect::<Vec<_>>());
    client.close().await.unwrap();
}

#[tokio::test]
async fn refusing_a_later_batch_item_rolls_back_earlier_inserts() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open(directory.path().join("purses.sqlite3"))
        .await
        .unwrap();
    let result = client
        .create_purses(vec!["should roll back".into(), String::new()], tag())
        .await;
    assert!(matches!(result, Err(Error::Refused(_))));
    assert_eq!(client.purses_tagged(tag()).await.unwrap(), Vec::new());
    assert_eq!(
        client
            .create_purse("first committed".into(), tag())
            .await
            .unwrap(),
        Some(PurseId(0))
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn committed_purses_survive_connection_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("purses.sqlite3");
    let client = Client::open(&path).await.unwrap();
    let id = client
        .create_purse("persistent".into(), tag())
        .await
        .unwrap()
        .unwrap();
    client.close().await.unwrap();
    let reopened = Client::open(&path).await.unwrap();
    assert_eq!(
        reopened.purse(id).await.unwrap(),
        Some(PurseRecord {
            id,
            name: "persistent".into(),
            tag: tag(),
        })
    );
    reopened.close().await.unwrap();
}

struct BrokenQuery;

impl Operation for BrokenQuery {
    const NAME: &'static str = "test.broken-query.v1";
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| Ok(connection.execute_batch("SELECT * FROM missing_table")?))
    }
}

#[tokio::test]
async fn sql_error_does_not_poison_later_operations() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open(directory.path().join("purses.sqlite3"))
        .await
        .unwrap();
    assert!(matches!(
        client.call(BrokenQuery).await,
        Err(Error::Sqlite(_))
    ));
    assert_eq!(
        client
            .create_purse("after error".into(), tag())
            .await
            .unwrap(),
        Some(PurseId(0))
    );
    client.close().await.unwrap();
}

struct Gate {
    entered: oneshot::Sender<std::thread::ThreadId>,
    release: mpsc::Receiver<()>,
}

impl Operation for Gate {
    const NAME: &'static str = "test.gate.v1";
    type Output = ();

    fn execute(self, _db: &mut Db) -> Result<Self::Output, Error> {
        let _ = self.entered.send(std::thread::current().id());
        self.release
            .recv()
            .map_err(|error| Error::Transport(error.to_string()))
    }
}

#[tokio::test]
async fn close_drains_admitted_jobs_and_stops_new_admission() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("purses.sqlite3");
    let client = Client::open(&path).await.unwrap();
    let (entered, arrived) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let mut running = Box::pin(client.call(Gate {
        entered,
        release: wait,
    }));
    assert!(running.as_mut().now_or_never().is_none());
    assert_ne!(arrived.await.unwrap(), std::thread::current().id());

    let mut create = Box::pin(client.create_purse("queued before close".into(), tag()));
    assert!(create.as_mut().now_or_never().is_none());
    let mut close = Box::pin(client.close());
    assert!(close.as_mut().now_or_never().is_none());
    let mut another_close = Box::pin(client.close());
    assert!(another_close.as_mut().now_or_never().is_none());
    assert_eq!(client.purse(PurseId(0)).await, Err(Error::Closed));

    release.send(()).unwrap();
    running.await.unwrap();
    assert_eq!(create.await.unwrap(), Some(PurseId(0)));
    close.await.unwrap();
    another_close.await.unwrap();
    let reopened = Client::open(&path).await.unwrap();
    assert_eq!(
        reopened.purse(PurseId(0)).await.unwrap(),
        Some(PurseRecord {
            id: PurseId(0),
            name: "queued before close".into(),
            tag: tag(),
        })
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn dropping_an_admitted_reply_does_not_retract_its_write() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open_with_readers(directory.path().join("purses.sqlite3"), 0)
        .await
        .unwrap();
    let (entered, arrived) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let mut running = Box::pin(client.call(Gate {
        entered,
        release: wait,
    }));
    assert!(running.as_mut().now_or_never().is_none());
    arrived.await.unwrap();
    let mut abandoned = Box::pin(client.create_purse("admitted".into(), tag()));
    assert!(abandoned.as_mut().now_or_never().is_none());
    drop(abandoned);
    release.send(()).unwrap();
    running.await.unwrap();
    assert_eq!(
        client.purses_tagged(tag()).await.unwrap(),
        vec![PurseRecord {
            id: PurseId(0),
            name: "admitted".into(),
            tag: tag(),
        }]
    );
    client.close().await.unwrap();
}

struct PanicDuringWrite;

impl Operation for PanicDuringWrite {
    const NAME: &'static str = "test.panic-during-write.v1";
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.write(|transaction| {
            transaction.execute("INSERT INTO nft_purse VALUES (0, 'uncommitted', x'')", [])?;
            panic!("requested panic after insert")
        })
    }
}

#[tokio::test]
async fn panic_rolls_back_the_write_and_settles_queued_calls() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("purses.sqlite3");
    let client = Client::open_with_readers(&path, 0).await.unwrap();
    let (entered, arrived) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let mut running = Box::pin(client.call(Gate {
        entered,
        release: wait,
    }));
    assert!(running.as_mut().now_or_never().is_none());
    arrived.await.unwrap();
    let mut failing = Box::pin(client.call(PanicDuringWrite));
    assert!(failing.as_mut().now_or_never().is_none());
    let mut queued = Box::pin(client.purse(PurseId(0)));
    assert!(queued.as_mut().now_or_never().is_none());
    release.send(()).unwrap();
    running.await.unwrap();
    assert!(matches!(failing.await, Err(Error::OutcomeUnknown(_))));
    assert!(matches!(queued.await, Err(Error::OutcomeUnknown(_))));
    let reopened = Client::open_with_readers(&path, 0).await.unwrap();
    assert_eq!(reopened.purse(PurseId(0)).await.unwrap(), None);
    reopened.close().await.unwrap();
}

struct Release(mpsc::Sender<()>);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn gate() -> (Gate, oneshot::Receiver<std::thread::ThreadId>, Release) {
    let (entered, arrived) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    (
        Gate {
            entered,
            release: wait,
        },
        arrived,
        Release(release),
    )
}

async fn finish<Output>(future: impl core::future::Future<Output = Output>) -> Output {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .expect("independent database work did not progress")
}

struct RenameWhileHeld {
    id: PurseId,
    gate: Gate,
}

impl Operation for RenameWhileHeld {
    const NAME: &'static str = "test.rename-while-held.v1";
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<(), Error> {
        db.write(|transaction| {
            transaction.execute(
                "UPDATE nft_purse SET name = 'committed' WHERE id = ?1",
                [self.id.0],
            )?;
            let _ = self.gate.entered.send(std::thread::current().id());
            self.gate
                .release
                .recv()
                .map_err(|error| Error::Transport(error.to_string()))
        })
    }
}

#[tokio::test]
async fn readers_see_committed_state_while_the_writer_is_in_a_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open(directory.path().join("purses.sqlite3"))
        .await
        .unwrap();
    let id = client
        .create_purse("before".into(), tag())
        .await
        .unwrap()
        .unwrap();
    let (gate, arrived, release) = gate();
    let mut write = Box::pin(client.call(RenameWhileHeld { id, gate }));
    assert!(write.as_mut().now_or_never().is_none());
    finish(arrived).await.unwrap();
    let before = finish(client.purse(id)).await.unwrap();
    drop(release);
    write.await.unwrap();
    let after = client.purse(id).await.unwrap();
    assert_eq!(
        (before, after),
        (
            Some(PurseRecord {
                id,
                name: "before".into(),
                tag: tag()
            }),
            Some(PurseRecord {
                id,
                name: "committed".into(),
                tag: tag()
            }),
        )
    );
    client.close().await.unwrap();
}

struct ReadGate(Gate);

impl Operation for ReadGate {
    const NAME: &'static str = "test.read-gate.v1";
    const ACCESS: Access = Access::Read;
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<(), Error> {
        self.0.execute(db)
    }
}

struct ReaderThread;

impl Operation for ReaderThread {
    const NAME: &'static str = "test.reader-thread.v1";
    const ACCESS: Access = Access::Read;
    type Output = std::thread::ThreadId;

    fn execute(self, _db: &mut Db) -> Result<Self::Output, Error> {
        Ok(std::thread::current().id())
    }
}

#[tokio::test]
async fn two_readers_execute_independently_and_rotate_across_calls() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open_with_readers(directory.path().join("purses.sqlite3"), 2)
        .await
        .unwrap();
    let (first_gate, first_arrived, first_release) = gate();
    let (second_gate, second_arrived, second_release) = gate();
    let mut first = Box::pin(client.call(ReadGate(first_gate)));
    let mut second = Box::pin(client.call(ReadGate(second_gate)));
    assert!(first.as_mut().now_or_never().is_none());
    let first_thread = finish(first_arrived).await.unwrap();
    assert!(second.as_mut().now_or_never().is_none());
    let second_thread = finish(second_arrived).await.unwrap();
    assert_ne!(first_thread, second_thread);
    drop(first_release);
    drop(second_release);
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(
        (
            client.call(ReaderThread).await.unwrap(),
            client.call(ReaderThread).await.unwrap()
        ),
        (first_thread, second_thread),
    );
    client.close().await.unwrap();
}

struct MisclassifiedWrite;

impl Operation for MisclassifiedWrite {
    const NAME: &'static str = "test.misclassified-write.v1";
    const ACCESS: Access = Access::Read;
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<(), Error> {
        db.read(|connection| {
            connection.execute("INSERT INTO nft_purse VALUES (0, 'forbidden', x'')", [])?;
            Ok(())
        })
    }
}

#[tokio::test]
async fn read_operations_cannot_write_and_the_reader_remains_usable() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open_with_readers(directory.path().join("purses.sqlite3"), 1)
        .await
        .unwrap();
    assert!(matches!(
        client.call(MisclassifiedWrite).await,
        Err(Error::Sqlite(_))
    ));
    assert_eq!(client.purse(PurseId(0)).await.unwrap(), None);
    assert_eq!(
        client.create_purse("allowed".into(), tag()).await.unwrap(),
        Some(PurseId(0))
    );
    client.close().await.unwrap();
}

#[tokio::test]
async fn pool_close_waits_for_both_readers_and_the_writer() {
    let directory = tempfile::tempdir().unwrap();
    let client = Client::open_with_readers(directory.path().join("purses.sqlite3"), 2)
        .await
        .unwrap();
    let (writer_gate, writer_arrived, writer_release) = gate();
    let (first_gate, first_arrived, first_release) = gate();
    let (second_gate, second_arrived, second_release) = gate();
    let mut writer = Box::pin(client.call(writer_gate));
    let mut first = Box::pin(client.call(ReadGate(first_gate)));
    let mut second = Box::pin(client.call(ReadGate(second_gate)));
    assert!(writer.as_mut().now_or_never().is_none());
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    finish(writer_arrived).await.unwrap();
    finish(first_arrived).await.unwrap();
    finish(second_arrived).await.unwrap();
    let mut close = Box::pin(client.close());
    assert!(close.as_mut().now_or_never().is_none());
    assert_eq!(client.purse(PurseId(0)).await, Err(Error::Closed));
    assert_eq!(
        client.create_purse("rejected".into(), tag()).await,
        Err(Error::Closed)
    );
    drop(writer_release);
    writer.await.unwrap();
    drop(first_release);
    first.await.unwrap();
    assert!(close.as_mut().now_or_never().is_none());
    drop(second_release);
    second.await.unwrap();
    finish(close).await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn private_memory_uses_one_connection_and_rejects_independent_readers() {
    assert!(Client::open_with_readers(":memory:", 2).await.is_err());
    let client = Client::open(":memory:").await.unwrap();
    let id = client
        .create_purse("memory".into(), tag())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client.purse(id).await.unwrap(),
        Some(PurseRecord {
            id,
            name: "memory".into(),
            tag: tag()
        })
    );
    client.close().await.unwrap();
}
