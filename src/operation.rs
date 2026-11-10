use rusqlite::{OptionalExtension, Row, Transaction, named_params};
use serde::{Deserialize, Serialize};

use crate::{Db, Error, PurseId, PurseRecord, PurseTag};

/// Selects the native connection that owns an operation.
pub enum Access {
    /// Runs on a read-only connection when readers are configured.
    Read,
    /// Runs on the single writer, including operations that both read and write.
    Write,
}

/// A complete synchronous database operation, with a stable wire identity.
pub trait Operation {
    /// Versioned name shared by caller and worker.
    const NAME: &'static str;
    /// Defaults to the writer; read operations must not change the database.
    const ACCESS: Access = Access::Write;
    /// Owned result returned after the operation completes.
    type Output;
    /// Executes beside SQLite; handlers must finish transactions before returning.
    fn execute(self, db: &mut Db) -> Result<Self::Output, Error>;
}

/// Reads one purse by identifier.
#[derive(Serialize, Deserialize)]
pub struct ReadPurse {
    /// Identifier to look up.
    pub id: PurseId,
}

impl Operation for ReadPurse {
    const NAME: &'static str = "nft.purse.v1";
    const ACCESS: Access = Access::Read;
    type Output = Option<PurseRecord>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT id, name, tag FROM nft_purse WHERE id = :id")?;
            Ok(statement
                .query_row(named_params! { ":id": self.id.0 }, purse_record)
                .optional()?)
        })
    }
}

/// Allocates one purse in a transaction.
#[derive(Serialize, Deserialize)]
pub struct CreatePurse {
    /// Display name.
    pub name: String,
    /// Grouping metadata.
    pub tag: PurseTag,
}

impl Operation for CreatePurse {
    const NAME: &'static str = "nft.create-purse.v1";
    type Output = Option<PurseId>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.write(|transaction| insert_purse(transaction, &self.name, &self.tag))
    }
}

/// Lists purses with matching opaque metadata.
#[derive(Serialize, Deserialize)]
pub struct PursesTagged {
    /// Exact tag to match.
    pub tag: PurseTag,
}

impl Operation for PursesTagged {
    const NAME: &'static str = "nft.purses-tagged.v1";
    const ACCESS: Access = Access::Read;
    type Output = Vec<PurseRecord>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT id, name, tag FROM nft_purse WHERE tag = :tag ORDER BY id",
            )?;
            let rows = statement.query_map(named_params! { ":tag": self.tag.0 }, purse_record)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }
}

/// Creates a batch atomically; an empty name refuses the entire batch.
#[derive(Serialize, Deserialize)]
pub struct CreatePurses {
    /// Names processed in order in one transaction.
    pub names: Vec<String>,
    /// Shared grouping metadata.
    pub tag: PurseTag,
}

impl Operation for CreatePurses {
    const NAME: &'static str = "nft.create-purses.v1";
    type Output = Vec<PurseId>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.write(|transaction| {
            self.names
                .into_iter()
                .map(|name| {
                    if name.is_empty() {
                        return Err(Error::Refused("purse name must not be empty".into()));
                    }
                    insert_purse(transaction, &name, &self.tag)?
                        .ok_or_else(|| Error::Refused("purse identifiers exhausted".into()))
                })
                .collect()
        })
    }
}

fn insert_purse(
    transaction: &Transaction<'_>,
    name: &str,
    tag: &PurseTag,
) -> Result<Option<PurseId>, Error> {
    let next: u32 = transaction
        .prepare_cached("SELECT COALESCE(MAX(id) + 1, 0) FROM nft_purse WHERE id < :main")?
        .query_row(named_params! { ":main": PurseId::MAIN.0 }, |row| row.get(0))?;
    if next == PurseId::MAIN.0 {
        return Ok(None);
    }
    transaction
        .prepare_cached("INSERT INTO nft_purse (id, name, tag) VALUES (:id, :name, :tag)")?
        .execute(named_params! { ":id": next, ":name": name, ":tag": tag.0 })?;
    Ok(Some(PurseId(next)))
}

fn purse_record(row: &Row<'_>) -> rusqlite::Result<PurseRecord> {
    Ok(PurseRecord {
        id: PurseId(row.get("id")?),
        name: row.get("name")?,
        tag: PurseTag(row.get("tag")?),
    })
}

/// Exercises an SQL failure without invalidating subsequent operations.
#[derive(Serialize, Deserialize)]
pub struct SqlFailure;

impl Operation for SqlFailure {
    const NAME: &'static str = "proof.sql-failure.v1";
    type Output = ();

    fn execute(self, db: &mut Db) -> Result<(), Error> {
        db.read(|connection| {
            connection.execute("INSERT INTO missing_table VALUES (1)", [])?;
            Ok(())
        })
    }
}

/// Holds SQLite inside a test-server gate to prove worker isolation.
#[derive(Serialize, Deserialize)]
pub struct HeldQuery;

impl Operation for HeldQuery {
    const NAME: &'static str = "proof.held-query.v1";
    type Output = i64;

    fn execute(self, db: &mut Db) -> Result<i64, Error> {
        db.read(|connection| {
            Ok(connection.query_row("SELECT wait_for_release()", [], |row| row.get(0))?)
        })
    }
}
