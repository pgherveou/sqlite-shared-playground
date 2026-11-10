use crate::{Db, Error, purse};

/// Initializes every domain before the database accepts operations.
pub fn initialize_database(db: &mut Db) -> Result<(), Error> {
    purse::initialize(db)?;
    #[cfg(target_arch = "wasm32")]
    crate::proof::initialize(db)?;
    Ok(())
}

/// Composes domain handlers and the playground's diagnostic operations.
#[cfg(any(target_arch = "wasm32", test))]
pub fn registry() -> Result<crate::registry::Registry, Error> {
    let mut registry = crate::registry::Registry::new();
    purse::register(&mut registry)?;
    registry.register::<crate::proof::SqlFailure>()?;
    registry.register::<crate::proof::HeldQuery>()?;
    Ok(registry)
}
