#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc;

use futures_channel::oneshot;
use futures_util::{FutureExt, future::join_all};
use sqlite_shared_playground::{Client, Db, Error, Operation, PurseId, PurseRecord, PurseTag};

fn tag() -> PurseTag {
    PurseTag(b"native-proof".to_vec())
}

#[test]
fn concurrent_creates_keep_each_name_and_allocate_distinct_ids() {
    smol::block_on(async {
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
    });
}

#[test]
fn refusing_a_later_batch_item_rolls_back_earlier_inserts() {
    smol::block_on(async {
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
    });
}

#[test]
fn committed_purses_survive_connection_replacement() {
    smol::block_on(async {
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
    });
}

struct BrokenQuery;

impl Operation for BrokenQuery {
    const NAME: &'static str = "test.broken-query.v1";
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| Ok(connection.execute_batch("SELECT * FROM missing_table")?))
    }
}

#[test]
fn sql_error_does_not_poison_later_operations() {
    smol::block_on(async {
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
    });
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

#[test]
fn close_drains_admitted_jobs_and_stops_new_admission() {
    smol::block_on(async {
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
    });
}

#[test]
fn dropping_an_admitted_reply_does_not_retract_its_write() {
    smol::block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let client = Client::open(directory.path().join("purses.sqlite3"))
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
    });
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

#[test]
fn panic_rolls_back_the_write_and_settles_queued_calls() {
    smol::block_on(async {
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
        arrived.await.unwrap();
        let mut failing = Box::pin(client.call(PanicDuringWrite));
        assert!(failing.as_mut().now_or_never().is_none());
        let mut queued = Box::pin(client.purse(PurseId(0)));
        assert!(queued.as_mut().now_or_never().is_none());
        release.send(()).unwrap();
        running.await.unwrap();
        assert!(matches!(failing.await, Err(Error::OutcomeUnknown(_))));
        assert!(matches!(queued.await, Err(Error::OutcomeUnknown(_))));
        let reopened = Client::open(&path).await.unwrap();
        assert_eq!(reopened.purse(PurseId(0)).await.unwrap(), None);
        reopened.close().await.unwrap();
    });
}
