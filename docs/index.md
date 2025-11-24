# 🆉 Zord Documentation Hub

Welcome to the Zord knowledge base — a CC0-licensed chronicle devoted to indexing rare and exotic zats on the Zcash chain.  This folder mirrors the structure used by Casey Rodarmor's `ord` project: every major concern lives in its own focused document so contributors can dive directly to the layer they care about.

## Table of Contents

1. [Architecture](architecture.md) – system diagram, components, and data flow
2. [Indexer & Metaprotocols](indexing.md) – ord-compatible inscription parsing plus ZRC-20 and ZNS semantics
3. [Security & Privacy](security.md) – defaults, hardening, and open risks
4. [Deployment & Operations](deployment.md) – Docker/Coolify notes, configuration, and troubleshooting

Each document is intentionally verbose and heavily cross-referenced so new reviewers can trace every decision back to the codebase.  When in doubt, follow the “measure twice, inscribe once” mantra: verify behavior from the RPC boundary down to the UI widgets that display the final inscription.
