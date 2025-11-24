# Architecture – OPI-Inspired Indexing for Zats

Zord borrows heavily from the `ord` layout: a single binary spawns an indexer task and an HTTP API, and isolated “engines” interpret metaprotocols from the same inscription stream.  Everything is written in Rust with a strict dependency boundary:

```
┌─────────────────────────────────┐
│  Zcash RPC + ZMQ notifications  │
└─────────────┬──────────────────┘
              │
      ┌───────▼───────┐
      │   Indexer     │    tokio task polling RPC + optional ZMQ triggers
      └───┬─────┬─────┘
          │     │
  ┌───────▼─┐ ┌─▼───────┐
  │ ZRC-20  │ │  ZNS    │   metaprotocol engines consume parsed inscriptions
  └─────────┘ └─────────┘
          │     │
      ┌───▼─────▼───┐
      │    Db (redb) │   durable state keyed by ids/tickers/addresses
      └──────┬──────┘
             │
      ┌──────▼──────┐
      │ HTTP API    │   Axum routes + static web surface
      └─────────────┘
```

## Key Components

### `ZcashRpcClient` (`src/rpc.rs`)
- Pulls credentials from `ZCASH_RPC_*` env vars; URL/username still default to Zatoshi’s public node for quick demos, but production deployments must override them.
- Builds a `reqwest::Client` with 30s timeout and Basic Auth header.
- Offers helper RPC calls used by the indexer.

### `Indexer` (`src/indexer.rs`)
- Maintains a streaming loop: read the latest DB height, compare with chain height, fetch blocks sequentially.
- Supports optional ZMQ notifications; when a push arrives we short-circuit the sleep and immediately poll for the next block.
- `parse_inscription` searches `scriptSig` assemblies for the ord-style envelope, strips DER signatures/public keys, and returns metadata ready for downstream engines.
- Emits high-signal tracing lines so production logs reveal every failure reason.

### `Db` (`src/db.rs`)
- Backed by [redb](https://crates.io/crates/redb); tables are typed and opened exactly once per transaction.
- Provides pagination helpers for inscriptions, tokens, and names so the UI can stay responsive even with millions of entries.
- Balance and token state is stored as JSON strings for now (mirroring ord), but is shielded behind typed helper structs so we can migrate to a binary format later.

### `Zrc20Engine` / `NamesEngine`
- Enforce metaprotocol invariants (ticker length, decimal math, first-come-first-serve naming) before the DB layer is touched.
- All numeric math is performed with checked `u128` intermediates to avoid silent overflow when dealing with 18-decimal assets.

### HTTP/API Layer (`src/api.rs`)
- Exposes REST endpoints under `/api/v1/...` for the new front-end components.
- Keeps ord-compatible routes (`/inscription/:id`, `/content/:id`, etc.) for parity with Bitcoin tooling.
- Serves the static `web/` assets at `/static/...`, while `/` is a curated landing page that loads the component library.

## Data Model Cheatsheet

| Table | Key | Value | Purpose |
|-------|-----|-------|---------|
| `blocks` | `u64 height` | `&str hash` | Track the tip the indexer has processed. |
| `inscriptions` | `&str id` | `&str metadata_json` | Raw inscription payloads (content + provenance). |
| `inscription_numbers` | `u64` | `&str id` | Deterministic numbering order. |
| `address_inscriptions` | `&str address` | `&str json_array` | Reverse lookup for wallet views. |
| `tokens` | `&str ticker` | `&str info_json` | ZRC-20 deployments. |
| `balances` | `&str address:ticker` | `&str Balance JSON` | Available vs overall holdings. |
| `names` | `&str name_lower` | `&str data_json` | ZNS entries. |

The schema is intentionally append-friendly: every write is scoped to a single short-lived redb transaction so we can rotate or rebuild parts of the index without exclusive locks.
