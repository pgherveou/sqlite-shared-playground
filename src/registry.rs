use std::collections::{BTreeMap, btree_map::Entry};

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    CreatePurse, CreatePurses, Db, Error, HeldQuery, Operation, PursesTagged, ReadPurse, SqlFailure,
};

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
        Work: Operation + DeserializeOwned,
        Work::Output: Serialize,
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

fn execute<Work>(payload: &[u8], db: &mut Db) -> Result<Vec<u8>, Error>
where
    Work: Operation + DeserializeOwned,
    Work::Output: Serialize,
{
    let (work, remaining) = postcard::take_from_bytes::<Work>(payload)
        .map_err(|error| Error::Protocol(format!("invalid operation payload: {error}")))?;
    if !remaining.is_empty() {
        return Err(Error::Protocol("trailing operation payload".into()));
    }
    let output = work.execute(db)?;
    postcard::to_allocvec(&output).map_err(|error| {
        Error::OutcomeUnknown(format!("result encoding failed after execution: {error}"))
    })
}

/// Registers the purse domain and the proof's diagnostic operations.
pub fn registry() -> Result<Registry, Error> {
    let mut registry = Registry::new();
    registry.register::<ReadPurse>()?;
    registry.register::<CreatePurse>()?;
    registry.register::<PursesTagged>()?;
    registry.register::<CreatePurses>()?;
    registry.register::<SqlFailure>()?;
    registry.register::<HeldQuery>()?;
    Ok(registry)
}

#[cfg(test)]
mod tests;
