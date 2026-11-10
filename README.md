# Shared SQLite playground

The same Rust database operations run on native connection threads and in a
separate browser database worker. This playground uses purse queries adapted
from `host-rust-core` to demonstrate both paths. It has no `async-sqlite`
dependency and is not a drop-in replacement for that crate.

## How async-sqlite works

[`async-sqlite`](https://github.com/ryanfowler/async-sqlite) owns each SQLite
connection on a native thread. The caller sends a Rust closure to that thread
and awaits its result. The closure can capture application values and use
ordinary `rusqlite` queries and transactions.

For an opened client, an existing purse table and an `id: u32`:

```rust
use rusqlite::OptionalExtension;

let name: Option<String> = sqlite.conn(move |conn| {
    conn.query_row(
        "SELECT name FROM nft_purse WHERE id = ?1",
        [id],
        |row| row.get(0),
    ).optional()
}).await?;
```

This executor relies on native threads and shared process memory. In a browser,
`std::thread::spawn` does not create a Web Worker. A separate worker has its own
WASM instance and cannot receive an arbitrary Rust closure and its captures.
Making the call `async` does not bridge that boundary.

## Shared native and WASM architecture

The caller submits a typed, named operation. Its Rust implementation lives beside
SQLite and can execute several queries within one transaction.

```text
Native application                 Browser UI
        |                               |
        |                         Rust/WASM caller worker
        |                               |
     PurseDao                        PurseDao
        |                               |
      Client                          Client
        | Rust value                    | operation name + SCALE arguments
        v                               v
Writer + reader threads            Dedicated DB worker, separate WASM instance
        |                               | registered handler
        v                               v
 Operation::execute                 Operation::execute
        |                               |
     SQLite                          SQLite / OPFS
```

- **Native:** send the operation value to a connection thread and execute it
  directly. No serialization or operation registry is needed.
- **WASM:** register the operation handlers when the database worker starts.
  The caller sends the operation name and arguments encoded with SCALE
  (`parity-scale-codec`). The worker looks up the handler, decodes the arguments,
  runs the operation and sends back a SCALE-encoded result or error. Only data
  crosses the boundary; the handler code is already compiled into the worker.

Both paths expose the same domain API. Given a connected `client` and `id: u32`:

```rust
use sqlite_shared_playground::{PurseDao, PurseId};

let purses = PurseDao::new(client.clone());
let name = purses.get(PurseId(id)).await?.map(|purse| purse.name);
```

[purse.rs](src/purse.rs) holds the DAO, domain types, operation implementations,
SQL and registration. [startup.rs](src/startup.rs) composes domain initialization
and registers the purse handlers with `purse::register(&mut registry)?`.
The client and executors remain independent of the domain.

Database work occupies only its connection thread or database worker. The browser
UI and Rust caller worker remain free to process messages while SQLite runs.

## Contracts and limits

### Trade-offs versus async-sqlite

- Database work needs named operations and worker registration instead of inline
  closures. Handlers still use ordinary `rusqlite`, including cached statements
  and multi-query transactions that finish before replying.
- Native pooling uses one writer and read-only readers. There is no global
  ordering across connections; await a write before a dependent read.
- Errors retain SQLite messages, not the full `rusqlite::Error` structure.
- Lifecycle behavior differs: accepted work survives dropped reply futures, close
  drains it, and an unwinding native panic stops that connection until reopening.

### Browser limitations

- Requests and results must cross as SCALE-encoded data. Captured Rust services
  cannot travel with them; caller and worker need compatible operation definitions.
- This proof uses one database connection. A long operation delays other database
  work, while the UI and caller worker remain responsive. Queues are unbounded
  and there is no execution deadline.
- Persistence requires OPFS support, with no fallback to memory. Coordinating
  database access across multiple tabs is not implemented.
- Terminating the worker or losing a reply can leave a write's outcome unknown.
  There are no automatic retries.

## Run

Requires Rust with `wasm32-unknown-unknown`, `wasm-pack`, and Node.js 20+.

```sh
cargo run --bin demo -- purses.sqlite3
npm ci
npm run build
npm run dev
```

Open <http://127.0.0.1:5191>. Run native tests with `cargo test --all-targets`.
After building WASM and installing Chromium (`npx playwright install chromium`),
run `npm test` for the browser tests, including worker isolation and OPFS persistence.
