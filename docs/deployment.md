# Deployment & Operations

## Configuration

| Variable | Default | Notes |
|----------|---------|-------|
| `ZCASH_RPC_URL` | `https://rpc.zatoshi.market/api/rpc` | Swap for your private node in production. |
| `ZCASH_RPC_USERNAME` | `zatoshi` | Same advice as above. |
| `ZCASH_RPC_PASSWORD` | _none_ | Required.  Process exits if missing. |
| `API_PORT` | `8080` | Set to `3333` for the 135.181.6.234 Coolify target. |
| `DB_PATH` | `./data/index` (dev) / `/data/zord.db` (container) | Mount persistent storage here. |
| `ZSTART_HEIGHT` | `3132356` | Block height of the first ord-style envelope on Zcash. |
| `ZMQ_URL` | unset | Optional `tcp://host:port` for low-latency tips. |
| `VERBOSE_LOGS` | `false` | Set to `true` to enable debug-level tracing in production. |

## Docker

```
# Build + run locally
API_PORT=3333 docker compose up -d --build
curl http://localhost:3333/health
```

Notes:
- The health-check probes `/health`, matching the Axum route.
- `zord-data` volume stores the redb database.
- Logs are emitted to stdout in JSON if `RUST_LOG` is set to `info,axum::rejection=warn`.

## Coolify @ 135.181.6.234

1. Add a new “Dockerfile” service pointing at this repository and branch.
2. Set environment variables (especially `ZCASH_RPC_PASSWORD`) inside Coolify secrets.
3. Map external port `3333` → container port `3333` and expose it via your chosen domain/IP.
4. Configure the health-check endpoint as `http://127.0.0.1:3333/health`.
5. Attach a persistent volume (e.g., `/opt/zord/data:/data`).

## Manual Rollout Checklist

1. `cargo fmt && cargo check` locally.
2. `API_PORT=3333 docker compose up --build` to ensure the container runs with the chosen port.
3. Tail logs until you see `Starting API on port 3333` and `Indexed block ...` lines.
4. Run smoke tests:
   - `curl http://server:3333/health`
   - `curl http://server:3333/api/v1/inscriptions?limit=1`
5. Announce readiness only after the indexer catches up to the current chain tip.

## Observability

- `tracing` spans record every indexed block, every inscription type, and RPC failures.
- In production, pipe stdout through something like `journald` or `vector` and configure alerts on the absence of “Indexed block” lines for >5 minutes.

## Disaster Recovery

- The database is append-friendly; keep periodic snapshots of `/data` (LVM, ZFS, or rsync) to recover quickly.
- If the DB becomes corrupted, delete the directory and restart the binary—the indexer will rescan from `ZSTART_HEIGHT`.
