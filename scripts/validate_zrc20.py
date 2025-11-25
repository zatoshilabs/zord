#!/usr/bin/env python3
"""Simple validation script for the Zord HTTP interface.

Usage examples:
    python scripts/validate_zrc20.py --base http://127.0.0.1:3000
    python scripts/validate_zrc20.py --base http://127.0.0.1:3000 --tick zatz --address t1example...
"""
from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict


def fetch_json(base: str, path: str) -> Dict[str, Any]:
    url = urllib.parse.urljoin(base, path)
    try:
        with urllib.request.urlopen(url, timeout=10) as resp:
            if resp.status != 200:
                raise RuntimeError(f"{url} -> HTTP {resp.status}")
            data = json.loads(resp.read().decode('utf-8'))
            return data
    except urllib.error.URLError as exc:  # pragma: no cover - network
        raise RuntimeError(f"Failed to fetch {url}: {exc}") from exc


def validate_tokens(base: str) -> None:
    data = fetch_json(base, '/api/v1/tokens?page=0&limit=200')
    items = data.get('items', [])
    if not items:
        raise RuntimeError('No tokens returned by /api/v1/tokens')

    problems = []
    for token in items:
        ticker = token.get('ticker')
        supply = token.get('supply')
        max_supply = token.get('max')
        progress = token.get('progress')
        if not ticker:
            problems.append('Missing ticker in token entry')
        if supply is None or max_supply is None:
            problems.append(f'{ticker or "unknown"}: missing supply/max fields')
        if progress is None or not (0.0 <= float(progress) <= 1.0):
            problems.append(f'{ticker or "unknown"}: invalid progress {progress}')

    if problems:
        raise RuntimeError('\n'.join(problems))

    print(f"✓ {len(items)} tokens returned")


def validate_tick(base: str, tick: str) -> None:
    data = fetch_json(base, f'/token/{tick}')
    if 'error' in data:
        raise RuntimeError(f"Token {tick} not found: {data['error']}")
    required = ['max', 'lim', 'dec', 'supply']
    missing = [field for field in required if field not in data]
    if missing:
        raise RuntimeError(f"Token {tick} missing fields: {', '.join(missing)}")
    print(f"✓ token/{tick} -> supply {data['supply']}")


def validate_balance(base: str, tick: str, address: str) -> None:
    data = fetch_json(base, f'/token/{tick}/balance/{address}')
    if 'error' in data:
        raise RuntimeError(f"Balance query error: {data['error']}")
    print(
        "✓ balance",
        data['tick'],
        data['address'],
        'available',
        data['available'],
        'overall',
        data['overall'],
    )


def main() -> int:
    parser = argparse.ArgumentParser(description='Validate Zord API responses')
    parser.add_argument('--base', default='http://127.0.0.1:3000', help='Base URL of the server')
    parser.add_argument('--tick', help='Ticker to inspect (lowercase)')
    parser.add_argument('--address', help='Optional address for balance check (requires --tick)')
    args = parser.parse_args()
    base = args.base.rstrip('/')

    status = fetch_json(base, '/api/v1/status')
    print('✓ status height', status.get('height'), 'inscriptions', status.get('inscriptions'))

    validate_tokens(base)

    if args.tick:
        tick = args.tick.lower()
        validate_tick(base, tick)
        if args.address:
            validate_balance(base, tick, args.address)

    print('Validation complete')
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except RuntimeError as exc:
        print(f'ERROR: {exc}', file=sys.stderr)
        raise SystemExit(1)
