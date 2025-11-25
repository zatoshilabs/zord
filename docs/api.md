# Zord API Documentation

Zord provides a JSON API for querying inscriptions, ZRC-20 tokens, and ZNS names on Zcash.

## Base URL
All API endpoints are prefixed with `/api/v1`.

## Endpoints

### Status
Get the current indexer status.
- **GET** `/status`
- **Response**:
  ```json
  {
    "height": 3147101,
    "inscriptions": 87078,
    "tokens": 125,
    "names": 602,
    "synced": true,
    "version": "0.1.0"
  }
  ```

### Inscriptions
Get a paginated list of recent inscriptions.
- **GET** `/inscriptions`
- **Parameters**:
  - `page` (optional, default 0): Page number.
  - `limit` (optional, default 24): Items per page.
- **Response**:
  ```json
  {
    "page": 0,
    "limit": 24,
    "total": 87078,
    "has_more": true,
    "items": [ ... ]
  }
  ```

### Tokens (ZRC-20)
Get a list of deployed tokens.
- **GET** `/tokens`
- **Parameters**:
  - `page` (optional, default 0)
  - `limit` (optional, default 100)
  - `q` (optional): Search query (prefix match on ticker).
- **Response**:
  ```json
  {
    "items": [
      {
        "ticker": "zero",
        "max": "21000000",
        "supply": "0",
        "deployer": "...",
        "progress": 0.0
      }
    ]
  }
  ```

### Names (ZNS)
Get a list of registered names.
- **GET** `/names`
- **Parameters**:
  - `page` (optional, default 0)
  - `limit` (optional, default 100)
  - `q` (optional): Search query (prefix match on name).
- **Response**:
  ```json
  {
    "items": [
      {
        "name": "satoshi.zec",
        "owner": "...",
        "inscription_id": "..."
      }
    ]
  }
  ```

## Legacy / Ord-Compatible Endpoints
For compatibility with existing tooling:
- `/inscription/:id` - Get inscription metadata/HTML.
- `/content/:id` - Get raw inscription content.
- `/token/:tick` - Get specific token info.
- `/token/:tick/balance/:address` - Get token balance.
