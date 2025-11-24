# Security & Privacy Notes

Security must be considered from the RPC boundary through to the DOM that renders inscriptions.  This document consolidates every known risk and the mitigations currently in place.

## Credentials & RPC Defaults

- `ZCASH_RPC_PASSWORD` is mandatory – the binary refuses to boot without it.
- `ZCASH_RPC_URL` and `ZCASH_RPC_USERNAME` fall back to Zatoshi's public node to support zero-config demos.  Production deployments **must** supply their own RPC endpoint; the public one is rate-limited and not intended for mainnet indexing.
- Callers should set `RUST_LOG=info` or `warn` in production so secrets never appear in structured logs.

## Overflow-Safe Token Math

- All ZRC-20 math is performed in `u128` before being downcast to `u64`.
- `parse_amount` fails with a descriptive error if a human-readable amount would overflow 64-bit storage.
- Mint and transfer handlers double-check balance availability and total supply before mutating state.

## XSS & HTML Injection

- Ord-compatible HTML responses now escape every user-controlled field via `html_escape::encode_text`.
- The new `web/` UI uses Web Components that render text via `textContent`, avoiding `innerHTML` injection paths.
- Binary content previews are explicitly labeled and must be downloaded; we never render untrusted bytes inline without an accompanying content type.

## Availability

- `/status` no longer walks one million inscriptions; it reads the tracked counter which is updated as part of every inscription transaction.
- Pagination APIs execute bounded DB iterations (offset + limit) so malicious clients cannot request “everything” in a single response.
- Tokio tasks log and back off for five seconds when encountering RPC failures to avoid hot-looping a downed node.

## Still on the Radar

1. **Chain reorg handling** – we currently assume the chain tip is final; adding a short reorg buffer will eliminate rare consistency edge cases.
2. **Transfer inscription UTXO tracking** – without it we cannot definitively prove asset movement, though we do prevent replays via DB state.
3. **Rate limiting** – the public API does not yet enforce per-IP quotas.  Place it behind a reverse proxy if exposure to the open internet is expected.

Contributions that improve any of these areas should be documented in this file so future reviewers can trace the threat model evolution.
