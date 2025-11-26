#!/usr/bin/env python3
"""HTTP smoke tests for all Zord endpoints.

Discovers sample data from the API, then probes every route defined in src/api.rs
using realistic parameters. Prints a compact report and exits non‑zero on failure.

Usage:
  python scripts/smoke_endpoints.py --base http://127.0.0.1:3000
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, List, Optional, Tuple


def http_get(url: str, timeout: int = 15) -> Tuple[int, Dict[str, str], bytes]:
    req = urllib.request.Request(url, headers={"User-Agent": "zord-smoke/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # nosec - controlled domains
        status = resp.status
        headers = {k.lower(): v for k, v in resp.headers.items()}
        body = resp.read()
        return status, headers, body


def fetch_json(url: str, timeout: int = 15) -> Dict[str, Any]:
    status, _headers, body = http_get(url, timeout=timeout)
    if status != 200:
        raise RuntimeError(f"{url} -> HTTP {status}")
    try:
        return json.loads(body.decode("utf-8"))
    except json.JSONDecodeError as exc:  # pragma: no cover - network
        raise RuntimeError(f"{url} -> invalid JSON: {exc}") from exc


def encode_path(path: str) -> str:
    """Percent-encode a path, preserving separators and query string.

    - Encodes each path segment individually to support non-ASCII tickers/names.
    - Leaves query string untouched assuming it is ASCII-safe (keys/values already encoded).
    """
    if "?" in path:
        route, query = path.split("?", 1)
    else:
        route, query = path, ""
    segs = route.split("/")
    enc_route = "/".join(urllib.parse.quote(seg, safe="-_.~") for seg in segs)
    return enc_route + ("?" + query if query else "")


def build_url(base: str, path: str) -> str:
    return base + encode_path(path)


def main() -> int:
    ap = argparse.ArgumentParser(description="Probe all Zord endpoints")
    ap.add_argument("--base", default="http://127.0.0.1:3000", help="Base URL of the server")
    ap.add_argument("--timeout", type=int, default=20, help="Per-request timeout (s)")
    args = ap.parse_args()
    base = args.base.rstrip("/")

    start = time.time()
    failures: List[str] = []

    # 1) Discover sample data for parameterized routes
    try:
        status = fetch_json(build_url(base, "/api/v1/status"), timeout=args.timeout)
        print(f"✓ status height={status.get('height')} tokens={status.get('tokens')} names={status.get('names')}")
    except Exception as e:
        failures.append(f"/api/v1/status -> {e}")

    tokens_feed = fetch_json(build_url(base, "/api/v1/tokens?page=0&limit=50"))
    tokens: List[Dict[str, Any]] = tokens_feed.get("items", [])
    sample_tick: Optional[str] = tokens[0]["ticker"] if tokens else None

    # For ZRC20 balance/rank/address tests, try to pick a holder of the sample token
    sample_address: Optional[str] = None
    if sample_tick:
        try:
            balances = fetch_json(build_url(base, f"/api/v1/zrc20/token/{sample_tick}/balances?page=0&limit=25"))
            holders = balances.get("holders", [])
            if holders:
                sample_address = holders[0].get("address")
        except Exception:
            pass

    # Names discovery
    names_feed = fetch_json(build_url(base, "/api/v1/names?page=0&limit=50"))
    names: List[Dict[str, Any]] = names_feed.get("items", [])
    sample_name: Optional[str] = names[0]["name"] if names else None
    sample_name_owner: Optional[str] = names[0]["owner"] if names else None

    # Inscriptions discovery
    inscriptions = fetch_json(build_url(base, "/api/v1/inscriptions"))
    inscription_items: List[Dict[str, Any]] = inscriptions if isinstance(inscriptions, list) else inscriptions.get("items", [])
    sample_inscription: Optional[Dict[str, Any]] = inscription_items[0] if inscription_items else None
    sample_inscription_id: Optional[str] = sample_inscription.get("id") if sample_inscription else None
    sample_txid: Optional[str] = None
    if sample_inscription and isinstance(sample_inscription.get("meta"), dict):
        sample_txid = sample_inscription["meta"].get("txid")

    # Block height
    height_json = fetch_json(build_url(base, "/block/height"))
    sample_height = height_json.get("height")

    # ZRC721 discovery
    zrc721_status = fetch_json(build_url(base, "/api/v1/zrc721/status"))
    zrc721_collections = fetch_json(build_url(base, "/api/v1/zrc721/collections?page=0&limit=25"))
    collections: List[Dict[str, Any]] = zrc721_collections.get("collections", [])
    sample_collection: Optional[str] = collections[0]["collection"] if collections else None
    sample_nft_id: Optional[str] = None
    if sample_collection:
        try:
            tokens_resp = fetch_json(build_url(base, f"/api/v1/zrc721/collection/{sample_collection}/tokens?page=0&limit=10"))
            collection_tokens: List[Dict[str, Any]] = tokens_resp.get("tokens", [])
            if collection_tokens:
                sample_nft_id = collection_tokens[0].get("token_id")
        except Exception:
            pass

    # Try to find a valid transfer inscription id
    sample_transfer_id: Optional[str] = None
    for item in inscription_items[:25]:
        iid = item.get("id")
        if not iid:
            continue
        try:
            tr = fetch_json(build_url(base, f"/api/v1/zrc20/transfer/{iid}"))
            if isinstance(tr, dict) and not tr.get("error"):
                sample_transfer_id = iid
                break
        except Exception:
            # ignore and continue scanning
            continue

    # 2) Build the full endpoint list
    endpoints: List[Tuple[str, str]] = []  # (path, kind)

    # Static pages
    endpoints += [
        ("/", "html"),
        ("/tokens", "html"),
        ("/names", "html"),
        ("/names/zec", "html"),
        ("/names/zcash", "html"),
        ("/collections", "html"),
        ("/zrc721", "html"),
        ("/docs", "html"),
        ("/spec", "html"),
        ("/api", "html"),
    ]

    # JSON feeds
    endpoints += [
        ("/api/v1/inscriptions", "json"),
        ("/api/v1/tokens?page=0&limit=24", "json"),
        ("/api/v1/names?page=0&limit=24", "json"),
        ("/api/v1/names/zec?page=0&limit=24", "json"),
        ("/api/v1/names/zcash?page=0&limit=24", "json"),
        ("/api/v1/status", "json"),
        ("/api/v1/zrc20/status", "json"),
        ("/api/v1/zrc20/tokens?page=0&limit=24", "json"),
        ("/api/v1/zrc721/status", "json"),
        ("/api/v1/zrc721/collections?page=0&limit=24", "json"),
        ("/api/v1/healthz", "json"),
    ]

    # Parametrized routes with discovered samples
    if sample_tick:
        endpoints += [
            (f"/token/{sample_tick}", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}/summary", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}/balances?page=0&limit=10", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}/integrity", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}/burned", "json"),
        ]

    addr_for_tests = sample_address or sample_name_owner
    if sample_tick and addr_for_tests:
        endpoints += [
            (f"/token/{sample_tick}/balance/{addr_for_tests}", "json"),
            (f"/api/v1/zrc20/address/{addr_for_tests}", "json"),
            (f"/api/v1/zrc20/token/{sample_tick}/rank/{addr_for_tests}", "json"),
        ]

    if sample_collection:
        endpoints += [
            (f"/collection/{sample_collection}", "html"),
            (f"/api/v1/zrc721/collection/{sample_collection}", "json"),
            (f"/api/v1/zrc721/collection/{sample_collection}/tokens?page=0&limit=10", "json"),
        ]
    if sample_collection and sample_nft_id:
        endpoints.append((f"/api/v1/zrc721/token/{sample_collection}/{sample_nft_id}", "json"))

    if sample_name:
        endpoints += [
            (f"/name/{sample_name}", "json"),
            (f"/resolve/{sample_name}", "json"),
            (f"/api/v1/resolve/{sample_name}", "json"),
        ]

    if sample_name_owner:
        endpoints.append((f"/api/v1/names/address/{sample_name_owner}", "json"))

    if sample_inscription_id:
        endpoints += [
            (f"/inscription/{sample_inscription_id}", "html"),
            (f"/content/{sample_inscription_id}", "bytes"),
            (f"/preview/{sample_inscription_id}", "html"),
        ]
    if sample_transfer_id:
        endpoints.append((f"/api/v1/zrc20/transfer/{sample_transfer_id}", "json"))

    if sample_height is not None:
        endpoints.append((f"/block/{sample_height}", "json"))

    if sample_txid:
        endpoints.append((f"/tx/{sample_txid}", "json"))

    if addr_for_tests:
        endpoints.append((f"/address/{addr_for_tests}/inscriptions", "json"))

    # Compatibility and misc
    endpoints += [
        ("/status", "json"),
        ("/health", "json"),
    ]

    # 3) Probe endpoints
    ok_count = 0
    seen = set()
    for path, kind in endpoints:
        if path in seen:
            continue
        seen.add(path)
        url = build_url(base, path)
        try:
            status, headers, body = http_get(url, timeout=args.timeout)
            if status != 200:
                failures.append(f"{path} -> HTTP {status}")
                print(f"✗ {path} [{status}]")
                continue
            if kind == "json":
                try:
                    data = json.loads(body.decode("utf-8"))
                except Exception:
                    failures.append(f"{path} -> invalid JSON")
                    print(f"✗ {path} [invalid JSON]")
                    continue
                if isinstance(data, dict) and data.get("error"):
                    failures.append(f"{path} -> error: {data.get('error')}")
                    print(f"✗ {path} [error: {data.get('error')}]")
                    continue
            # For HTML/bytes we just care about status 200
            size = len(body)
            ctype = headers.get("content-type", "?")
            print(f"✓ {path} [{ctype}; {size} bytes]")
            ok_count += 1
        except urllib.error.URLError as e:
            failures.append(f"{path} -> network error: {e}")
            print(f"✗ {path} [network error: {e.reason}]")
        except Exception as e:  # pragma: no cover - network variability
            failures.append(f"{path} -> {e}")
            print(f"✗ {path} [{e}]")

    elapsed = time.time() - start
    print(f"---\nDone: {ok_count} OK, {len(failures)} failed in {elapsed:.1f}s")
    if failures:
        for f in failures:
            print(f"FAIL: {f}")
        return 2
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("Interrupted", file=sys.stderr)
        raise SystemExit(130)
