use std::collections::{BTreeMap, btree_map::Entry};

use parity_scale_codec::{Decode, DecodeAll, Encode};

use crate::{Db, Error, Operation};

type Handler = fn(&[u8], &mut Db) -> Result<Vec<u8>, Error>;

/// Maps stable operation names to typed decoding and execution.
pub struct Registry {
    handlers: BTreeMap<&'static str, Handler>,
}

impl Registry {
    /// Starts an explicit application operation catalog.
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
        }
    }

    /// Adds a handler, rejecting ambiguous protocol names.
    pub fn register<Work>(&mut self) -> Result<(), Error>
    where
        Work: Operation + Decode,
        Work::Output: Encode,
    {
        match self.handlers.entry(Work::NAME) {
            Entry::Vacant(entry) => {
                entry.insert(execute::<Work>);
                Ok(())
            }
            Entry::Occupied(_) => Err(Error::Protocol(format!(
                "duplicate operation: {}",
                Work::NAME
            ))),
        }
    }

    /// Validates a request before running its complete operation.
    pub fn dispatch(&self, name: &str, payload: &[u8], db: &mut Db) -> Result<Vec<u8>, Error> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| Error::Protocol(format!("unknown operation: {name}")))?;
        handler(payload, db)
    }
}

fn execute<Work>(mut payload: &[u8], db: &mut Db) -> Result<Vec<u8>, Error>
where
    Work: Operation + Decode,
    Work::Output: Encode,
{
    let work = Work::decode_all(&mut payload)
        .map_err(|error| Error::Protocol(format!("invalid operation payload: {error}")))?;
    Ok(work.execute(db)?.encode())
}

#[cfg(test)]
mod tests;
