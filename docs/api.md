# Zord HTTP API

This document describes the public JSON API exposed by Zord for:

- Global status and blockchain snapshot
- Inscriptions feed and previews
- ZRC-20 (fungible) tokens: listings, balances, transfers
- ZRC-721 (NFT) collections and tokens
- ZNS (.zec /.zcash) name resolution

Base path: `/api/v1` unless otherwise stated.

Notes on amounts
- Amounts returned by balance endpoints are base units (strings). Use `dec` to scale to human units: human = base / 10^dec.
- Integrity endpoint returns base units for exact comparisons.

## Global / Blockchain
- GET `/api/v1/status` → `{ height, chain_tip, inscriptions, tokens, names, components:{core,zrc20,names}, version }`
- GET `/block/height` → `{ height }` (latest indexed block height)
- GET `/block/:query` → block by height or hash `{ hash, height, time, tx, previous }`
- GET `/tx/:txid` → raw transaction `{ txid, hex, vin:[{txid,vout}], vout:[{n,value,addresses}] }`

## Inscriptions
- GET `/api/v1/inscriptions?page=&limit=` → paginated feed with content types, sizes, sender labels, and previews.
- Compat HTML/bytes:
  - GET `/inscription/:id` (HTML detail)
  - GET `/preview/:id` (framed preview)
  - GET `/content/:id` (raw bytes)

## ZRC-20 (fungible)
- List tokens
  - GET `/api/v1/tokens?page=&limit=&q=` → `{ items:[ { ticker, max, max_base_units, supply, supply_base_units, lim, dec, deployer, inscription_id, progress } ] }`
- Token info
  - GET `/api/v1/zrc20/token/:tick` → stored deploy record `{ tick, max, lim, dec, deployer, supply(base units), inscription_id }`
  - GET `/api/v1/zrc20/token/:tick/summary` → `{ holders, transfers_completed, supply_base_units, lim, max, dec, integrity:{ consistent, sum_holders_base_units } }`
- Holders for a ticker
  - GET `/api/v1/zrc20/token/:tick/balances?page=&limit=` → `{ tick, page, limit, total_holders, holders:[ { address, available, overall } ] }`
- Address portfolio
  - GET `/api/v1/zrc20/address/:address` → `{ address, balances:[ { tick, available, overall } ] }`
  - Rank/percentile within a ticker: GET `/api/v1/zrc20/token/:tick/rank/:address` → `{ rank, total_holders, percentile }`
- Transfer inspection
  - GET `/api/v1/zrc20/transfer/:id` → `{ inscription_id, transfer:{ tick, amt, sender }, used, outpoint? }`
- Integrity
  - GET `/api/v1/zrc20/token/:tick/integrity` → `{ supply_base_units, sum_overall_base_units, sum_available_base_units, total_holders, consistent }`
- Status
  - GET `/api/v1/zrc20/status` → `{ height, chain_tip, tokens, version }`
- Compatibility
  - GET `/token/:tick` (same as token info, legacy)
  - GET `/token/:tick/balance/:address`

## ZRC-721 (NFT)
- Collections
  - GET `/api/v1/zrc721/collections?page=&limit=` → `{ collections:[ { collection, supply, minted, meta, royalty, deployer, inscription_id } ] }`
  - GET `/api/v1/zrc721/collection/:collection` → deploy record
- Tokens
  - GET `/api/v1/zrc721/collection/:collection/tokens?page=&limit=` → `{ tokens:[ { collection, token_id, owner, inscription_id, metadata, metadata_path } ] }`
  - GET `/api/v1/zrc721/address/:address` → `{ tokens:[ ... ] }`
- Status
  - GET `/api/v1/zrc721/status` → `{ collections, tokens, height, chain_tip, version }`
- Deploy/mint payloads (indexer rules)
  - Deploy: `{ "p":"zrc-721","op":"deploy","collection":"ZGODS","supply":"10000","meta":"<cid or object>","royalty":"100" }`
  - Mint: `{ "p":"zrc-721","op":"mint","collection":"ZGODS","id":"0" }`
  - Rules: first‑is‑first; ids are numeric and 0 ≤ id < supply.

## Names (ZNS)
- List (all): GET `/api/v1/names?page=&limit=&q=&tld=zec|zcash`
- List (.zec): GET `/api/v1/names/zec?page=&limit=&q=`
- List (.zcash): GET `/api/v1/names/zcash?page=&limit=&q=`
- Names by owner: GET `/api/v1/names/address/:address`
- Resolve: GET `/api/v1/resolve/:name` → `{ name, address }` or `{ error }`
  - Also available at `/resolve/:name` (browser convenience)

## Examples
- ZERO holders sum:
  ```sh
  curl -s '/api/v1/zrc20/token/zero/balances?page=0&limit=20000' \
    | jq -r '[.holders[].overall|tonumber]|add'
  ```
- Integrity:
  ```sh
  curl -s '/api/v1/zrc20/token/zero/integrity' | jq
  ```
- Address balances:
  ```sh
  curl -s '/api/v1/zrc20/address/t1...' | jq
  ```
