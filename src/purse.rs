use parity_scale_codec::{Decode, Encode};
use rusqlite::{OptionalExtension, Row, Transaction, named_params};

use crate::{Access, Client, Db, Error, Operation};

/// An NFT purse identifier; the maximum value is reserved for the main purse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseId(pub u32);

impl PurseId {
    /// The built-in purse, excluded from allocated identifiers.
    pub const MAIN: Self = Self(u32::MAX);
}

/// Application metadata matched as opaque bytes.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseTag(pub Vec<u8>);

/// A purse returned by the shared operations.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PurseRecord {
    /// Stable database identifier.
    pub id: PurseId,
    /// Caller-provided display name.
    pub name: String,
    /// Opaque grouping metadata.
    pub tag: PurseTag,
}

/// Asynchronous purse queries shared by native and browser callers.
pub struct PurseDao {
    client: Client,
}

impl PurseDao {
    /// Uses the client's shared executor and lifecycle.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Creates a purse under the next free identifier.
    pub async fn create(&self, name: String, tag: PurseTag) -> Result<Option<PurseId>, Error> {
        self.client.call(CreatePurse { name, tag }).await
    }

    /// Reads one purse, returning `None` when absent.
    pub async fn get(&self, id: PurseId) -> Result<Option<PurseRecord>, Error> {
        self.client.call(ReadPurse { id }).await
    }

    /// Lists matching purses in identifier order.
    pub async fn tagged(&self, tag: PurseTag) -> Result<Vec<PurseRecord>, Error> {
        self.client.call(PursesTagged { tag }).await
    }

    /// Commits an entire batch or rolls it back on refusal.
    pub async fn create_batch(
        &self,
        names: Vec<String>,
        tag: PurseTag,
    ) -> Result<Vec<PurseId>, Error> {
        self.client.call(CreatePurses { names, tag }).await
    }
}

/// Initializes purse storage without changing existing records.
pub fn initialize(db: &mut Db) -> Result<(), Error> {
    db.write(|transaction| {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS nft_purse (
                 id INTEGER PRIMARY KEY CHECK (id BETWEEN 0 AND 4294967295),
                 name TEXT NOT NULL,
                 tag BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS nft_purse_tag ON nft_purse(tag);
             INSERT OR IGNORE INTO nft_purse (id, name, tag)
                 VALUES (4294967295, 'Main', x'');",
        )?;
        Ok(())
    })
}

/// Registers the domain's operations on the database worker.
#[cfg(any(target_arch = "wasm32", test))]
pub fn register(registry: &mut crate::registry::Registry) -> Result<(), Error> {
    registry.register::<ReadPurse>()?;
    registry.register::<CreatePurse>()?;
    registry.register::<PursesTagged>()?;
    registry.register::<CreatePurses>()?;
    Ok(())
}

/// Reads one purse by identifier.
#[derive(Encode, Decode)]
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
#[derive(Encode, Decode)]
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
#[derive(Encode, Decode)]
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
#[derive(Encode, Decode)]
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
