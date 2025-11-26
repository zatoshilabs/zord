#!/usr/bin/env python3
"""Check ZRC-20 integrity across all tickers.

Fetches the token list, then queries /api/v1/zrc20/token/:tick/integrity for each
and prints rows where supply != sum_overall. Exit non-zero if any drift is found.

Usage:
  python scripts/check_integrity.py --base http://127.0.0.1:8080
"""
from __future__ import annotations

import argparse
import json
import sys
import urllib.parse
import urllib.request
from typing import Any, Dict


def fetch_json(url: str) -> Dict[str, Any]:
    with urllib.request.urlopen(url, timeout=20) as resp:
        if resp.status != 200:
            raise RuntimeError(f"HTTP {resp.status}: {url}")
        return json.loads(resp.read().decode('utf-8'))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--base', default='http://127.0.0.1:8080', help='Base URL')
    ap.add_argument('--limit', default=500, type=int, help='Max tokens to scan')
    args = ap.parse_args()
    base = args.base.rstrip('/')

    tokens = fetch_json(f"{base}/api/v1/tokens?page=0&limit={args.limit}")
    items = tokens.get('items', [])
    if not items:
        print('No tokens returned; nothing to check')
        return 0

    drift = []
    for item in items:
        tick = item.get('ticker')
        if not tick:
            continue
        data = fetch_json(f"{base}/api/v1/zrc20/token/{tick}/integrity")
        if data.get('error'):
            print(f"skip {tick}: {data['error']}")
            continue
        if not data.get('consistent', False):
            drift.append(data)

    if drift:
        print('Integrity drift detected:')
        for d in drift:
            print(json.dumps(d, indent=2))
        return 2

    print(f"OK: {len(items)} tokens consistent")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

