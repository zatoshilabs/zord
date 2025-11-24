# Zord – Rare & Exotic Zats Indexer

Zord is a Rust indexer for Zcash ordinals (“Zats”) with first-class support for ZRC-20 tokens and ZNS names.  The project follows Casey Rodarmor’s `ord` conventions, is released under CC0-1.0, and ships an opinionated web UI plus documented APIs so explorers can track inscriptions in real time.

## Quick Start

```bash
# native
cargo run --release

# docker compose
API_PORT=3333 docker compose up -d
```

Environment variables live in `.env.example`.  Set `ZCASH_RPC_PASSWORD` before running; URL/username default to the public Zatoshi RPC for demos but should be overridden in production.

## Documentation

All long-form docs moved to [`/docs`](docs/index.md):

- [Architecture](docs/architecture.md)
- [Indexer & Metaprotocols](docs/indexing.md)
- [Security](docs/security.md)
- [Deployment](docs/deployment.md)

## Web UI & API

- Landing page: `/` (served from `web/` and powered by lightweight Web Components)
- REST feeds: `/api/v1/inscriptions`, `/api/v1/tokens`, `/api/v1/names`
- Ord-compatible routes: `/inscription/:id`, `/content/:id`, `/preview/:id`

Use these endpoints to build explorers, dashboards, or downstream indexers focused on rare and exotic zats.
