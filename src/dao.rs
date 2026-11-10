use rusqlite::{Connection, OptionalExtension, Row, Transaction, named_params};

use crate::{Error, PurseId, PurseRecord, PurseTag};

const NEXT_PURSE_ID: &str = "SELECT COALESCE(MAX(id) + 1, 0) FROM nft_purse WHERE id < :main";
const INSERT_PURSE: &str = "INSERT INTO nft_purse (id, name, tag) VALUES (:id, :name, :tag)";

/// Allocates and inserts within the caller's transaction.
pub fn insert_purse(
    tx: &Transaction<'_>,
    name: &str,
    tag: &PurseTag,
) -> Result<Option<PurseId>, Error> {
    let next: u32 = tx
        .prepare_cached(NEXT_PURSE_ID)?
        .query_row(named_params! { ":main": PurseId::MAIN.0 }, |row| row.get(0))?;
    if next == PurseId::MAIN.0 {
        return Ok(None);
    }
    tx.prepare_cached(INSERT_PURSE)?
        .execute(named_params! { ":id": next, ":name": name, ":tag": tag.0 })?;
    Ok(Some(PurseId(next)))
}

const PURSES_TAGGED: &str = "SELECT id, name, tag FROM nft_purse WHERE tag = :tag ORDER BY id";

/// Returns matching purses in allocation order.
pub fn purses_tagged(conn: &Connection, tag: &PurseTag) -> Result<Vec<PurseRecord>, Error> {
    let mut statement = conn.prepare_cached(PURSES_TAGGED)?;
    let rows = statement.query_map(named_params! { ":tag": tag.0 }, purse_record)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

const PURSE: &str = "SELECT id, name, tag FROM nft_purse WHERE id = :id";

/// Returns a purse if its identifier exists.
pub fn purse(conn: &Connection, id: PurseId) -> Result<Option<PurseRecord>, Error> {
    let mut statement = conn.prepare_cached(PURSE)?;
    Ok(statement
        .query_row(named_params! { ":id": id.0 }, purse_record)
        .optional()?)
}

fn purse_record(row: &Row<'_>) -> rusqlite::Result<PurseRecord> {
    Ok(PurseRecord {
        id: PurseId(row.get("id")?),
        name: row.get("name")?,
        tag: PurseTag(row.get("tag")?),
    })
}
