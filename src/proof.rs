use parity_scale_codec::{Decode, Encode};

use crate::{Db, Error, Operation};
#[cfg(target_arch = "wasm32")]
use {rusqlite::functions::FunctionFlags, wasm_bindgen::prelude::*};

/// Exercises an SQL failure without invalidating subsequent operations.
#[derive(Encode, Decode)]
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
#[derive(Encode, Decode)]
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

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = "
export function waitForRelease() {
    const request = new XMLHttpRequest();
    request.open('GET', '/query-gate', false);
    request.send();
    if (request.status !== 200) throw new Error('query gate failed');
}
")]
extern "C" {
    #[wasm_bindgen(catch, js_name = waitForRelease)]
    fn wait_for_release() -> Result<(), JsValue>;
}

/// Installs the browser isolation test's SQLite gate.
#[cfg(target_arch = "wasm32")]
pub fn initialize(db: &mut Db) -> Result<(), Error> {
    db.read(|connection| {
        connection.create_scalar_function(
            "wait_for_release",
            0,
            FunctionFlags::SQLITE_UTF8,
            |_| {
                wait_for_release().map_err(|error| {
                    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                        error.as_string().unwrap_or_else(|| format!("{error:?}")),
                    )))
                })?;
                Ok(0_i64)
            },
        )?;
        Ok(())
    })
}
