# Shared SQLite playground

The same Rust purse operations run on native connection threads
and in a dedicated browser database worker. There is no `async-sqlite` dependency.
This is an execution-model proof, not a drop-in replacement for that crate.
The native demo and async tests use Tokio 1, matching host-rust-core. SQLite runs
on its dedicated connection threads; browser tasks use `wasm-bindgen-futures`.
Worker requests, results and errors use SCALE (`parity-scale-codec` 3).

```text
Native Rust CLI                 Browser page
      |                              |
      |                        Rust/WASM caller worker
      |                              |
   Client                         Client
      | typed job                    | encoded operation
      v                              v
Writer + reader threads         Database worker, separate WASM instance
      |                              | Registry
      +--------- Operation::execute(&mut Db)
                              |
                       rusqlite / SQLite
```

## How it differs from async-sqlite

[`async-sqlite`](https://github.com/ryanfowler/async-sqlite) sends a Rust closure
to a connection-owning native thread. The closure can use ordinary `rusqlite`
queries, transactions and captured application values. The following read assumes
an existing purse table:

```rust
use rusqlite::OptionalExtension;

let sqlite = async_sqlite::ClientBuilder::new()
    .path("purses.sqlite3")
    .open()
    .await?;

let name: Option<String> = sqlite.conn(move |conn| {
    conn.query_row(
        "SELECT name FROM nft_purse WHERE id = ?1",
        [id],
        |row| row.get(0),
    ).optional()
}).await?;
```

Native threads share memory, so they can receive that closure. A separate browser
worker has its own WASM instance and cannot receive arbitrary captured Rust
closures. Making the call `async` does not remove this boundary.

Here, the caller invokes a named operation through a typed facade:

```rust
let client = Client::open("purses.sqlite3").await?; // native
let name = client.purse(PurseId(id)).await?.map(|purse| purse.name);
```

The operation's arguments and implementation are declared once:

```rust
#[derive(Encode, Decode)]
pub struct ReadPurse {
    pub id: PurseId,
}

impl Operation for ReadPurse {
    const NAME: &'static str = "nft.purse.v1";
    const ACCESS: Access = Access::Read;
    type Output = Option<PurseRecord>;

    fn execute(self, db: &mut Db) -> Result<Self::Output, Error> {
        db.read(|conn| {
            let mut statement = conn
                .prepare_cached("SELECT id, name, tag FROM nft_purse WHERE id = :id")?;
            Ok(statement
                .query_row(named_params! { ":id": self.id.0 }, purse_record)
                .optional()?)
        })
    }
}
```

`Client::purse(id)` calls `Client::call(ReadPurse { id })`. Native dispatch sends
that value to a reader thread and invokes `execute` directly, without
serialization. Browser dispatch sends the operation name and encoded arguments;
the database worker decodes them with SCALE and invokes the same implementation.
Only the final result crosses back. Worker startup registers the handler explicitly:

```rust
registry.register::<ReadPurse>()?;
```

On WASM, construction is `Client::connect(worker).await`; the domain methods are
identical. There is no native registry lookup or serialization requirement.
Decoders reject trailing bytes. The envelope version and explicit enum indexes
define the wire format; caller and database worker must use compatible builds.

| | async-sqlite closure API | This playground |
| --- | --- | --- |
| Unit of work | Arbitrary Rust closure | Named Rust operation |
| Native execution | Connection thread, with optional pool | One writer and configurable read-only readers |
| Browser execution | Closure cannot cross independent WASM instances | Registered handler in a separate database worker |
| SQLite access | Ordinary `rusqlite` inside the closure | Ordinary `rusqlite` inside the handler |
| Application work | Write the closure at the call site | Declare an operation and register it for browser use |

A handler can use cached statements, map rows, and compose several queries
inside one `Db::write` transaction. Transactions finish before the handler replies;
there is no message per SQL statement or remote transaction handle. Application
services captured by existing closures still need an explicit design if the
operation depends on them. This proof covers data-only purse operations.

## Native readers and writer

Operations default to `Access::Write`. `ReadPurse` and `PursesTagged` declare
`Access::Read`; native dispatch selects a reader round-robin. Each connection has
its own thread. The writer initializes the schema and enables WAL before readers
open with SQLite's read-only flag. Operations mixing reads and writes stay on the
writer for their entire transaction.

```rust
let client = Client::open("purses.sqlite3").await?; // one writer, two readers
// Or choose the reader count; zero routes every operation to the writer.
let client = Client::open_with_readers("other.sqlite3", 4).await?;
```

Readers can progress during a write and see committed data. There is no global
FIFO across connections: await a write before submitting a dependent read. Close
stops admission to all connections and waits for their queued work. Private memory
and temporary databases default to one connection and reject explicit readers.
The browser uses the same operation definitions on its single database worker;
`Access` does not create additional browser workers. Read-only enforcement applies
to native reader connections; zero-reader mode and WASM use the writer connection.

## Run

Requires Rust with `wasm32-unknown-unknown`, `wasm-pack`, and Node.js 20+.

```sh
cargo run --bin demo -- purses.sqlite3
npm ci
npm run build
npm run dev
```

Open <http://127.0.0.1:5191>. The browser demo creates, fetches and lists purses,
demonstrates refused-batch rollback, and reopens OPFS storage.

```sh
cargo test --all-targets
npx playwright install chromium
npm test                       # after npm run build
cargo clippy --all-targets -- -D warnings
cargo clippy --lib --target wasm32-unknown-unknown -- -D warnings
cargo +nightly fmt --check
```

## Where to look

| File | Responsibility |
| --- | --- |
| `src/operation.rs` | Typed arguments, SQL, row decoding and shared operation bodies |
| `src/lib.rs` | Async domain facade |
| `src/db.rs` | Connection, schema and transaction wrapper |
| `src/native.rs` | Access routing, writer and reader threads, shared shutdown |
| `src/registry.rs` | Typed worker handlers, duplicate and unknown-name checks |
| `src/browser/` | Worker transport, correlation and lifecycle |
| `src/web_api.rs`, `web/` | Browser entry points and demo |

The purse queries, schema and domain types are adapted from `host-rust-core` at
`4b58ce1db4c01e937ebbb1e00c40833b99e5bfe5`, specifically `nft/dao.rs`,
`nft/purse.rs` and `store/migration/v2_to_v3_nft_purses.rs`. SQL and row decoding
are retained. Standalone imports/errors and serialization derives replace host
dependencies; schema initialization is idempotent. The batch refusal rule is a
proof-specific operation, not a rule added to the source DAO.

## Contracts and limits

Accepted operations continue if their waiting future is dropped. Close drains
accepted work before releasing storage. Termination or a lost reply can leave a
write's outcome unknown; the client never retries it automatically. An unwinding
native panic exits the affected connection thread naturally and closes pending
reply channels; there is no panic interception or automatic replacement. Calls
selecting that owner fail until the client is reopened. Errors use a deliberately
small serializable type; SQLite errors retain their message, not the full
`rusqlite::Error` structure.

The browser tests run real Chromium and OPFS. A test-only server gate holds a
SQLite UDF while the separate Rust caller answers a ping, proving worker
isolation without relying on an arbitrary query duration. Diagnostic operations
are part of this playground's catalog, not a proposed production API.

This proof uses unbounded queues and one browser database connection. It does not
implement subscriptions, schema upgrades, key allocation, durable
ownership coordination or multi-tab database sharing. Operations are trusted Rust
code and must complete their transactions through `Db::write`. A hung worker has
no transport deadline; callers may explicitly terminate it. Persistent browser
storage requires OPFS support; initialization errors do not fall back to memory.
