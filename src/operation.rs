use serde::{Deserialize, Serialize};

use crate::{Db, Error, PurseId, PurseRecord, PurseTag, dao};

/// A complete synchronous database operation, with a stable wire identity.
pub trait Operation {
    /// Versioned name shared by caller and worker.
    const NAME: &'static str;
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
    type Output = Option<PurseRecord>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| dao::purse(connection, self.id))
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
        db.write(|transaction| dao::insert_purse(transaction, &self.name, &self.tag))
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
    type Output = Vec<PurseRecord>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|connection| dao::purses_tagged(connection, &self.tag))
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
                    dao::insert_purse(transaction, &name, &self.tag)?
                        .ok_or_else(|| Error::Refused("purse identifiers exhausted".into()))
                })
                .collect()
        })
    }
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
