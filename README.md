# Shared SQLite playground

The same Rust purse DAO and operation handlers run on a native connection thread
and in a dedicated browser database worker. There is no `async-sqlite` dependency.

```text
Native Rust CLI                 Browser page
      |                              |
      |                        Rust/WASM caller worker
      |                              |
   Client                         Client
      | typed job                    | encoded operation
      v                              v
Connection thread               Database worker, separate WASM instance
      |                              | Registry
      +--------- Operation::execute(&mut Db)
                              |
                     DAO / rusqlite / SQLite
```

```rust
let client = Client::open("purses.sqlite3").await?;
let tag = PurseTag(vec![1, 2, 3]);
let id = client.create_purse("Savings".into(), tag.clone()).await?;
let purses = client.purses_tagged(tag).await?;
client.close().await?;
```

On WASM, construction is `Client::connect(worker).await`; the domain methods are
identical. Native dispatch has no serialization bounds. Browser startup explicitly
registers each operation once. A handler may call several DAO functions in one
transaction; neither statements nor transaction handles cross worker boundaries.

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
| `src/dao.rs` | Purse SQL, cached statements and row decoding |
| `src/operation.rs` | Typed arguments and shared operation bodies |
| `src/lib.rs` | Async domain facade |
| `src/db.rs` | Connection, schema and transaction wrapper |
| `src/native.rs` | Direct jobs on one owning thread |
| `src/registry.rs` | Typed worker handlers, duplicate and unknown-name checks |
| `src/browser/` | Worker transport, correlation and lifecycle |
| `src/web_api.rs`, `web/` | Browser entry points and demo |

The purse DAO, schema and domain types are adapted from `host-rust-core` at
`4b58ce1db4c01e937ebbb1e00c40833b99e5bfe5`, specifically `nft/dao.rs`,
`nft/purse.rs` and `store/migration/v2_to_v3_nft_purses.rs`. SQL and row decoding
are retained. Standalone imports/errors and serialization derives replace host
dependencies; schema initialization is idempotent. The batch refusal rule is a
proof-specific operation, not a rule added to the source DAO.

## Contracts and limits

Accepted operations continue if their waiting future is dropped. Close drains
accepted work before releasing storage. Termination or a lost reply can leave a
write's outcome unknown; the client never retries it automatically. Native panics
stop the connection owner. Errors use a deliberately small serializable type;
SQLite errors retain their message, not the full `rusqlite::Error` structure.

The browser tests run real Chromium and OPFS. A test-only server gate holds a
SQLite UDF while the separate Rust caller answers a ping, proving worker
isolation without relying on an arbitrary query duration. Diagnostic operations
are part of this playground's catalog, not a proposed production API.

This proof uses one connection per executor and unbounded queues. It does not
implement reader pools, subscriptions, schema upgrades, key allocation, durable
ownership coordination or multi-tab database sharing. Operations are trusted Rust
code and must complete their transactions through `Db::write`. A hung worker has
no transport deadline; callers may explicitly terminate it. Persistent browser
storage requires OPFS support; initialization errors do not fall back to memory.
