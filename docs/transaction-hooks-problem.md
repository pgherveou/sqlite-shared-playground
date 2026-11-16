# Extending another layer's transaction with a database worker

This document describes one pattern in `host-rust-core` that the shared
native and WASM approach of this playground cannot express as it stands. It
was raised in review:

> It looks like it requires all the actual querying logic do stay inside the
> dao? This might not work in the current form for cases where we want to
> execute some additional hook in the same db transaction like when the ledger
> allows upper layers to extend the db transaction with additional logic upon
> durable tx registration

## Summary

The durable transaction ledger of `host-rust-core` lets its caller run code
inside the ledger's own write transaction. The caller passes closures, and the
ledger calls them between its own SQL statements, so the rows of both layers
commit or roll back together.

This playground sends named operations made of data to the database. A closure
is code plus captured memory, so it cannot be sent to a database worker. The
SQL is never the problem. What breaks is everything in that transaction that is
not SQL:

1. Code chosen by the caller and run by the ledger.
2. A wallet service, holding a secret, that is called in the middle of the
   write.
3. The ledger's own in-memory state, updated before the commit.

## What the shared approach requires

Every database call is an [`Operation`](../src/operation.rs): a struct whose
`execute` is compiled into whatever runs beside SQLite.

```rust
pub trait Operation {
    const NAME: &'static str;
    const ACCESS: Access = Access::Write;
    type Output;
    fn execute(self, db: &mut Db) -> Result<Self::Output, Error>;
}
```

On native targets the operation value is moved to a connection thread. In the
browser the caller and the database live in two separate WASM instances, and
only bytes cross between them:

```rust
// caller side, browser (src/lib.rs)
pub async fn call<Work: Operation + Encode>(&self, work: Work) -> Result<Work::Output, Error>
where
    Work::Output: Decode,

// database worker (src/registry.rs)
pub fn register<Work>(&mut self) -> Result<(), Error>
where
    Work: Operation + Decode,
    Work::Output: Encode,
```

The worker looks an operation up by `NAME`, decodes its fields, and runs the
`execute` it was built with. It follows that:

- An operation's fields must be plain data.
- All code that runs in one transaction must already be compiled into the
  worker, under one operation.
- Nothing in the caller's memory is reachable from inside `execute`.

## What `host-rust-core` does today

Its store hands a closure to the thread that owns the SQLite writer
(`store.rs`, built on `async-sqlite`):

```rust
pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, DbError> + Send + 'static,
```

That thread shares the process's memory, so the closure may capture anything
and call anything.

The durable ledger builds on this to offer two extension points
(`durable/engine/registration.rs`):

```rust
// 1. The caller decides, inside the write, which transactions to register.
pub fn scheduled_in(
    domain: DomainId,
    group: Option<GroupId>,
    policy: PolicyId,
    plan: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<Vec<PolicyParams>, E> + Send + 'static,
) -> Self

// 2. The caller writes its own rows, inside the write, once the ids exist.
pub async fn execute_with<T, E, F>(
    self: &Arc<Self>,
    request: DurableRequest<E>,
    on_register: F,
) -> Result<(Vec<DurableTxId>, T), RegistrationError<E>>
where
    F: FnOnce(&rusqlite::Transaction<'_>, &[DurableTxId]) -> Result<T, E> + Send + 'static,
```

The ledger combines both with its own work into a single closure and sends it
to the writer thread (`DurableTxEngine::record`):

```rust
let write_rows_and_hook = move |tx: &rusqlite::Transaction<'_>| {
    let presigned = request.transactions.presigned().len();
    let ids = match request.insert_all(tx)? {          // runs `plan`, then the ledger's inserts
        Ok(ids) => ids,
        Err(refusal) => return Ok(Err(refusal)),
    };
    claims.lock().extend(claim_presigned(&ownership, &ids, presigned));
    Ok(on_register(tx, &ids).map(|value| (ids, value)))
};
let written = self.db.write_unless_refused(write_rows_and_hook).await;
```

The NFT ledger is the caller. Every registration it makes
(`start_transfers`, `start_imports`, `start_reclaims`, `start_returns` in
`nft/ledger.rs`) goes through these two extension points:

```rust
pub async fn start_transfers(&self, request: TransferRequest) -> Result<(), ..> {
    let items = request.transfer.inputs();
    let installation = self.current_installation().await?;
    let planned = self.planned_transfers(request, installation);   // stores the `plan` closure
    self.durable
        .execute_with(planned, move |tx, ids| record_inputs(tx, items, ids))
        .await
        .map(|_| ())
}

fn planned_in_write<Plan>(&self, group: GroupId, installation: InstallationId, plan: Plan) -> .. {
    let deriver = self.keys.clone();                               // Arc<dyn ItemKeyDeriver>
    let policy = PolicyId::new(TRANSFER_POLICY);
    DurableRequest::scheduled_in(nft_domain(), Some(group), policy, move |tx| {
        plan(tx, DestinationKeys { installation, deriver: deriver.as_ref() })
    })
}
```

## One registration, step by step

Example: move items `A` and `B` from purse 3 into purse 7.

```text
 NftLedger               DurableTxEngine            DB writer thread
 (upper layer)           (lower layer)              (owns the connection)
     |                         |                            |
     | execute_with(           |                            |
     |   planned,              |                            |
     |   on_register)          |                            |
     |------------------------>|                            |
     |                         | db.write(closure)          |
     |                         |--------------------------->|
     |                         |                            | BEGIN IMMEDIATE
     |                         |                            |
     |                         |                            | plan(tx)                     NFT code
     |                         |                            |   operation not registered yet?
     |                         |                            |   A and B idle in purse 3?
     |                         |                            |   next free index of purse 7?   5
     |                         |                            |   wallet derives keys at 5, 6   A', B'
     |                         |                            |   INSERT nft_item A', B'
     |                         |                            |
     |                         |                            | dao::schedule(tx) x2         ledger code
     |                         |                            |   INSERT durable_tx           ids 41, 42
     |                         |                            |
     |                         |                            | claim_presigned              ledger memory
     |                         |                            |
     |                         |                            | on_register(tx, [41, 42])    NFT code
     |                         |                            |   INSERT nft_consumption (A, 41), (B, 42)
     |                         |                            |
     |                         |                            | COMMIT
     |                         |<---------------------------|
     |                         | builds, signs and          |
     |                         | broadcasts 41 and 42       |
     |<------------------------|                            |
```

Data flows from the NFT layer to the ledger and back within the transaction:

- The ledger cannot insert its rows before `plan` has worked out the params,
  and only NFT code can work them out, by reading NFT tables.
- The NFT layer cannot record `(A, 41)` before id 41 exists, and only the
  ledger's insert produces it.

Each step must share the transaction with the others:

- Checking that `A` is idle and marking it spent: otherwise two transfers can
  both pass the check and spend `A` twice.
- Reading the next free index and inserting the keys derived for it: otherwise
  two allocations can take the same slot.
- The `durable_tx` rows and the `nft_consumption` rows that reference them: a
  crash between two commits would leave a transfer nothing points at, or an
  item locked by a transfer that does not exist.

## Why this does not work with a database worker

| | What the write relies on | Where | Why a worker does not have it |
|---|---|---|---|
| 1 | Code chosen by the caller and run by the ledger | `plan` in `DurableRequest::scheduled_in`, `on_register` in `DurableTxEngine::execute_with`, and the closure given to `Db::write` | A closure has no byte form. The worker runs only functions compiled into it and found by name. |
| 2 | The wallet, called between two SQL statements | `keys.derive_item_keys(&slots)` in `allocate_items_in` (`nft/item.rs`), reached through the `deriver` that `plan` captures | The deriver holds the root entropy on the caller's side. Item keys use hard derivation, so even a public key needs the secret. The slots to derive at are only known inside the transaction. |
| 3 | The ledger's memory, updated before the commit | `claim_presigned` over `Ownership` (`durable/ownership.rs`) in `DurableTxEngine::record` | `Ownership` is an in-memory map of the engine. A worker has its own memory, so a claim made there is seen by nobody. |
| 4 | Typed results and refusals | `execute_with<T, E, F>` returns the hook's own `T` and `E` | Results cross as bytes, so `T` and `E` must encode, and this playground has a single `Error` type. |

Point 1 is what the review comment describes. The ledger is generic over what
its caller does inside the write. With operations, the code of a transaction
has to be known when the worker is built, so a lower layer cannot run
"whatever the upper layer passes in".

Point 3 only concerns presigned registrations. The claim is taken inside the
write so that a recovery pass can never reach a committed row before the
submission watch that is about to follow it.

## What is not affected

- The SQL statements themselves. Each one would run unchanged in a worker.
- Composing several synchronous `&Transaction` helpers inside one operation,
  as `CreatePurses` does with `insert_purse` in [`purse.rs`](../src/purse.rs).
- The ledger's other database calls: status reads, the verdict compare-and-set,
  starting an attempt, abandoning a submission. Each passes a closure to the
  store, but that closure is the ledger's own code capturing plain data, so it
  becomes a named operation mechanically. Signing and chain reads already
  happen outside any write.

## Where it applies

The `durable` and `nft` modules of `host-rust-core` are native-only today, and
on native targets this playground moves the operation value to the connection
thread without encoding it. The problem appears when:

- those modules have to run in the browser, behind a database worker, or
- native code adopts the operation-only API as well, so that one
  implementation serves both targets.

## What a solution has to keep

- The rows of both layers commit or roll back as one transaction.
- The upper layer can decide what to register from state it reads in that
  transaction.
- The upper layer receives the ledger's new ids in that transaction.
- A refusal from either layer rolls back everything and broadcasts nothing.
- The ledger's checks before the write, and its claim and broadcast after the
  commit, happen for every registration.
- The wallet's secret stays out of the database worker.
- A recovery pass does not decide a presigned transaction ahead of its watch.
