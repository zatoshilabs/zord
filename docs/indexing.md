# Indexing & Metaprotocol Standards

## Inscription Discovery

1. Determine the candidate block height (`start_height` env var, default 3132356 – the first Zcash block containing ord-style envelopes).
2. Fetch block hash + full block JSON.
3. For every transaction, pull the verbose raw transaction.
4. Inspect each `vin.scriptSig.asm` for the ord envelope:
   - Parse each push
   - Decode the first push into a MIME content type string
   - Concatenate subsequent pushes until we reach DER signatures or public keys
   - Produce `(inscription_id, sender, receiver, content_type, content_utf8, content_hex)`
5. Persist the inscription metadata atomically so APIs can read it immediately.
6. Stream the metadata through metaprotocol engines (ZRC-20, ZNS, future protocols).

The parser is strict about DER signatures and pubkeys to avoid the THREE CLASSIC BUGS we hit early on:
- **Uppercase tickers were rejected** – tickers are normalized to lowercase for storage while the original case is preserved for display.
- **72-byte payloads were misidentified as signatures** – we now check for the DER prefix (`0x30`) instead of coarse byte-length filters.
- **Pubkeys spilled into JSON payloads** – we drop pushes that look like compressed/uncompressed keys when they appear near the script tail.

## ZRC-20

- Protocol string must equal `zrc-20`.
- `tick` is normalized to lowercase and limited to 4–5 UTF-8 bytes.
- All numeric fields (`max`, `lim`, `amt`) are parsed using checked `u128` arithmetic, then downcast to `u64` with explicit overflow errors.
- Deploy: writes token metadata and initializes supply.
- Mint: enforces per-mint limit and total cap, then increments balances.
- Transfer (inscribe): locks the specified amount until a transfer event proves where it landed.
- Transfer (finalize): verifies the transfer inscription was not replayed, updates balances, and flips its state to “used”.

Future work (documented for parity with ord): add full UTXO tracking so transfer inscriptions can be validated purely by transaction graph rather than optimistic receivers.

## ZNS (Zcash Name Service)

- Only `text/plain` inscriptions ending in `.zec` or `.zcash` are eligible.
- Names are case-insensitive but we preserve original casing for display.
- First inscription wins; duplicates are rejected with an error.
- Owners are derived from the first output address of the enclosing transaction (matching the behavior of early ordinals tooling).

## API Surfaces

- `/api/v1/inscriptions?page=0&limit=24` – JSON feed used by the UI components.
- `/api/v1/tokens` and `/api/v1/names` follow the same pagination contract.
- Legacy ord endpoints remain for backwards compatibility (`/inscription/:id`, `/content/:id`, `/preview/:id`).

Each JSON response advertises `page`, `has_more`, and `total` so the front-end can render paginators without guessing.
