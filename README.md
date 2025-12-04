# ZORD Protocol

**Ordinals‑style inscriptions, NFTs, and tokens on Zcash**
by **Zatoshi Labs**

---

## 1. Abstract

ZORD is an **ordinal-style metaprotocol for Zcash**. It serializes the smallest units of ZEC (“zatoshis” or *zats*), attaches arbitrary data to individual zats (“inscriptions”), and defines open token standards on top:

* **ZRC‑20** – BRC‑20–style fungible tokens for Zcash ([zatoshi.gitbook.io][1])
* **ZRC‑721** – Ordinals-style NFT inscriptions on Zcash ([zatoshi.gitbook.io][2])
* **ZNS** – Human‑readable namespaces like `anon.zec` or `1.zcash`

The protocol is implemented in **Zord**, a Rust indexer and explorer that follows Casey Rodarmor’s original **`ord`** conventions, adapted to Zcash.([GitHub][3])  ZORD *does not* change Zcash consensus: all rules live at the **indexing layer** and are enforced by wallets and marketplaces.

ZORD takes advantage of Zcash’s underlying privacy design: Zcash combines a Bitcoin‑style transparent UTXO set with shielded pools that hide sender, receiver, and amount using zk‑SNARKs.([Coinpare][4])  ZORD currently indexes **transparent** transactions only, but inscriptions and tokens can still benefit from Zcash’s privacy in practice when users keep addresses unlinkable to their real identities and use shielded pools for funding flows.

---

## 2. Design Goals

1. **Ordinal semantics on Zcash**

   * Assign a stable serial number to every *zat* (10⁻⁸ ZEC) and track it across the chain.
   * Attach arbitrary data to specific zats (inscriptions) that can be transferred and traded.

2. **Minimal L1 assumptions**

   * No consensus changes, no new opcodes, no forks.
   * Use existing Zcash script / transaction primitives only.

3. **Simple, JSON‑based token standards**

   * ZRC‑20 and ZRC‑721 are defined as plain UTF‑8 JSON inscriptions.([zatoshi.gitbook.io][1])
   * All state is derived from on‑chain inscription ownership. No smart contracts.

4. **Composable infrastructure**

   * Open‑source Rust indexer (`zord`) with HTTP APIs and ord‑compatible routes.([GitHub][5])
   * Zord explorer, Zatoshi RPC, Zatoshi mempool, Zatoshi Market, and Zatoshi Wallet built on the same index.

5. **Privacy‑aligned**

   * Do not weaken Zcash’s privacy guarantees.
   * Allow NFT and memecoin ownership that is *more* private than on non‑private L1s (Bitcoin ordinals, Solana, etc.), assuming good operational security.

---

## 3. Background

### 3.1 Ordinals on Bitcoin (very short version)

Casey Rodarmor’s **Ordinals** protocol assigns an ordinal number to each satoshi in the order mined and preserved across transactions. Inscriptions (data blobs) are attached to individual sats using a specific Taproot witness envelope, making sats collectible and tradable as NFTs or as `BRC‑20` fungible tokens.([GitHub][3])

Ordinals live purely as a **metaprotocol**: Bitcoin consensus is unaware of them.

### 3.2 Zcash: transparent & shielded

Zcash is a Bitcoin‑derived chain that adds **shielded pools** using zk‑SNARKs while retaining a transparent UTXO set similar to Bitcoin’s.([Coinpare][4])

* **Transparent UTXOs** use standard script types (`P2PKH`, `P2SH`).
* **Shielded transfers** hide sender, receiver, and amounts in zero‑knowledge.

Important architectural differences vs Bitcoin:

* Zcash **does not implement SegWit or Taproot**; transparent addresses are still classic P2PKH/P2SH.
* Zcash supports **OP_RETURN** in transparent scripts with an **80‑byte payload limit**, which is widely used for embedding short memos and metadata.([Zcash Community Forum][6])

These constraints mean we **cannot** simply replay the Taproot‑based ordinals encoding on Zcash. ZORD must use Zcash‑native ways of attaching data—primarily transparent script data such as `OP_RETURN`.

---

## 4. Ordinal Theory on Zcash (“Zats”)

### 4.1 Zatoshis

* 1 ZEC = 10⁸ zats (zatoshis), analogous to satoshis on Bitcoin.([Wikipedia][7])
* ZORD assigns each zat a unique **ordinal index** based on:

  1. Block height & position of coinbase outputs.
  2. Transaction ordering inside the block.
  3. Output index within each transaction.
  4. Spending / propagation rules (same as `ord` where applicable).

ZORD **follows `ord`’s ordinal assignment semantics** wherever they make sense, replacing Bitcoin’s monetary schedule and block interval with Zcash’s parameters (e.g., 21M cap, 75‑second block target).([GitHub][3])

The result is a deterministic mapping:

```text
(block_height, tx_index, output_index, value_in_zats)
  ⟶ contiguous range of ordinal indices
```

Any indexer implementing ZORD can reproduce the same mapping from raw Zcash blocks.

### 4.2 Ownership of zats

* At any point, each zat is owned by exactly one transparent UTXO.
* When the UTXO is spent, the zats move according to a deterministic assignment order (mirroring `ord`’s satoshi flow rules).
* ZORD tracks the **current owner** of any specific ordinal index.

---

## 5. Inscriptions

### 5.1 Conceptual model

An **inscription** is:

> *Arbitrary data bound to a specific zat at the moment that zat first reveals a valid ZORD inscription envelope on-chain.*

Intuitively:

1. A transaction publishes some inscription envelope (e.g., JSON with metadata or token ops).
2. ZORD ties that envelope to a specific zat in one of the transaction’s outputs.
3. Whoever controls the UTXO containing that zat **owns** the inscription.
4. Moving that UTXO transfers the inscription; destroying the UTXO (e.g. sending to unspendable script) burns it.

### 5.2 On‑chain encoding

Because Zcash lacks Taproot/witness data, ZORD uses **transparent script data** as the carrier:

* ZORD treats specific transparent outputs (e.g., `OP_RETURN` outputs) as inscription carriers.
* All bytes in the data push after the carrier opcode form the **inscription envelope**.

Given Zcash’s 80‑byte OP_RETURN limit, ZORD inscriptions are designed to be **small**:

* Token standards (ZRC‑20/ZRC‑721) use short UTF‑8 JSON envelopes that fit well within 80 bytes.([zatoshi.gitbook.io][1])
* Large media (images, animations) are usually stored off‑chain (e.g. IPFS) and referenced by CIDs or hashes in the JSON (see ZRC‑721 below).

> **Note on confirmation:**
> Public docs for `zord` and `zrc` describe inscriptions at the *envelope* level, not the exact script pattern. Given Zcash’s lack of Taproot and the documented 80‑byte OP_RETURN limit, OP_RETURN‑based carriers are the natural encoding for a practical implementation; this README focuses on the metaprotocol and envelope semantics, leaving script‑level details to the reference implementation.

### 5.3 Envelope format

At the ZORD level, an inscription envelope is:

* **Content**: UTF‑8 text or binary.
* **Content type**: Indicated via a MIME‑like string (e.g., `text/plain;charset=utf-8`, `application/json`).
* **Sub‑protocol**: Identified inside the envelope (e.g., `p: "zrc-20"`).

For tokens and namespaces, the envelope is **JSON** with a required `p` (protocol id) and `op` (operation) field.

---

## 6. ZRC Standards (Zord Repository of Coin Standards)

`ZRC` = **Zord Repository of Coin Standards** – a family of text‑inscription standards riding on top of ZORD.

### 6.1 ZRC‑20: Zcash Fungible Token Standard

ZRC‑20 is a minimal, BRC‑20–style fungible token standard for Zcash. Token state is tracked via JSON inscriptions with three operations: `deploy`, `mint`, and `transfer`. Indexers and wallets derive balances by following inscription ownership.([zatoshi.gitbook.io][1])

#### 6.1.1 Envelope format

* Payload: UTF‑8 JSON
* Recommended content type: `text/plain;charset=utf-8` (or `application/json`)
* Common fields:

  * `p`: must be `"zrc-20"`
  * `op`: operation (`"deploy"`, `"mint"`, `"transfer"`)
  * `tick`: token ticker (case‑sensitive, often 4 uppercase letters)
* All numeric fields are **stringified integers**.

Example:

```json
{
  "p": "zrc-20",
  "op": "deploy",
  "tick": "ZERO",
  "max": "21000000",
  "lim": "1000"
}
```

Only the **first valid `deploy`** for a given ticker is canonical; later deploy attempts with the same `tick` must be ignored by indexers.([zatoshi.gitbook.io][1])

#### 6.1.2 Operations

**Deploy**

Declares a token, its maximum supply, and the per‑mint cap:

```json
{
  "p": "zrc-20",
  "op": "deploy",
  "tick": "ZERO",
  "max": "21000000",
  "lim": "1000"
}
```

* `max`: total supply cap over the token’s lifetime.
* `lim`: maximum amount per `mint` inscription.

**Mint**

Creates fungible units up to the per‑mint cap and until `max` is reached:

```json
{
  "p": "zrc-20",
  "op": "mint",
  "tick": "ZERO",
  "amt": "1000"
}
```

* `amt` must be `<= lim`.
* Cumulative minted supply for the ticker must not exceed `max`.

Ownership rule:

> The holder of the *mint inscription’s UTXO* owns the minted amount. ([zatoshi.gitbook.io][1])

**Transfer**

Moves balances by inscribing a `transfer` and then sending the inscription UTXO to the recipient:

```json
{
  "p": "zrc-20",
  "op": "transfer",
  "tick": "ZERO",
  "amt": "500"
}
```

Transfer settlement is **UTXO‑based**:

* Balances update when the transfer inscription’s UTXO is sent to the recipient.
* Indexers track all `mint` and `transfer` inscriptions for a given `tick`, net them per UTXO owner, and compute balances.

#### 6.1.3 Indexing rules (normative)

For each token ticker `T`:

1. Find the earliest valid `deploy` inscription for `T`.
2. Consider all later `mint` and `transfer` inscriptions whose:

   * `p == "zrc-20"`, `tick == T`
   * JSON is syntactically valid and numeric fields are non‑negative integers.
3. Enforce **supply safety**:

   * Reject `mint` inscriptions when cumulative supply would exceed `max`.
4. Calculate balances:

   * Initialize balances at 0 for all addresses/UTXOs.
   * For each accepted `mint`, credit the owner of the mint inscription.
   * For each accepted `transfer`, debits and credits are applied when the inscription UTXO actually moves to a new address.

This procedure is deterministic and reproducible by any independent implementation.

---

### 6.2 ZRC‑721: Zcash NFT Inscription Standard

ZRC‑721 is a lightweight NFT inscription standard that blends BRC‑style minimal envelopes with ERC‑721‑like metadata conventions. Each `mint` inscription **is the NFT** (supply is always 1); there is no separate `transfer` operation beyond moving the inscription itself.([zatoshi.gitbook.io][2])

#### 6.2.1 Goals

* Minimal operations: `deploy` and `mint`
* Deterministic total supply and token IDs
* Off‑chain metadata & media via IPFS CIDs
* Optional royalty hint (basis points)
* Wallet / indexer friendly, open, permissionless

#### 6.2.2 Deploy inscription

Declares a collection and its metadata root:

```json
{
  "p": "zrc-721",
  "op": "deploy",
  "collection": "ZGODS",
  "supply": "10000",
  "meta": "bafybeicqjqzixdtawkbcuyaagrmk3vyfweidwzb6hwbucadhoxoe2pd3qm",
  "royalty": "100"
}
```

Fields:([zatoshi.gitbook.io][2])

* `p`: must be `"zrc-721"`
* `op`: `"deploy"`
* `collection`: case‑sensitive slug/name (short & unique)
* `supply`: stringified integer ≥ 1 (max tokens)
* `meta`: IPFS CID pointing to metadata folder root
* `royalty` (optional): secondary‑sale royalty in basis points (`"100"` = 1%). Intended as a hint for marketplaces; paid to the transparent address that inscribed the deploy.

Only the first valid `deploy` per `collection` is canonical; later deploys with the same name are ignored.

#### 6.2.3 Mint inscription

Creates a single NFT (supply 1) for a given collection:

```json
{
  "p": "zrc-721",
  "op": "mint",
  "collection": "ZGODS",
  "id": "0"
}
```

Fields:([zatoshi.gitbook.io][2])

* `p`: `"zrc-721"`
* `op`: `"mint"`
* `collection`: must match an existing `deploy`
* `id`: stringified integer, typically 0‑indexed `< supply`
* Each `id` can be minted **at most once**.

**Transfer model**

> The mint inscription UTXO *is* the NFT. Moving that UTXO (spending to a new address) moves the NFT. No explicit `transfer` op.

#### 6.2.4 Metadata and media

Metadata & imagery live on IPFS, anchored by the `meta` CID from the deploy. Recommended layout:([zatoshi.gitbook.io][2])

* Metadata JSON at: `ipfs://<meta_cid>/<id>.json`
* Inside each metadata JSON:

```json
{
  "name": "ZGODS 0",
  "collection": "ZGODS",
  "description": "The first ZRC-721 Inscription Collection on the ZCash Privacy Blockchain. We are the ZGODS, expect us.",
  "website": "https://example.com",
  "twitter": "https://x.com/example",
  "img": "ipfs://<image_cid>/0.png",
  "attributes": [
    { "trait_type": "Background", "value": "metro fiber junction" }
  ]
}
```

* `name`, `collection`, `description`, `img` strongly recommended.
* `attributes` follows standard ERC‑721/EIP‑1155 style trait arrays.
* Extra fields (links, creator info, animation URLs, etc.) are allowed and should remain stable once published.

---

### 6.3 ZNS – Zord Name Service

ZNS introduces **human‑readable names** on Zcash via simple text inscriptions:

* Names are plain UTF‑8 strings ending in `.zec` or `.zcash`.

  * Examples: `anon.zec`, `1.zec`, `zatoshi.zcash`
* Only the **first inscription** of a given string is canonical; later attempts are ignored by indexers, giving you a “first‑come, first‑served” name registry.

#### 6.3.1 Namespace inscription

A ZNS inscription is:

* Content type: `text/plain;charset=utf-8`
* Payload: the exact UTF‑8 name, e.g. `"anon.zec"`

Indexers:

1. Parse the inscription’s content as text.
2. If it ends with `.zec` or `.zcash` and has not been claimed before:

   * Register that name to the inscription’s owner.
3. Subsequent inscriptions with the same full string are non‑canonical.

#### 6.3.2 Ownership and transfer

As with NFTs:

* The UTXO containing the ZNS inscription owns the name.
* Spending that UTXO to a new address transfers the name.
* Sending it to a provably unspendable script (e.g. burn address) destroys the name.

Names can be used by:

* Wallets and explorers (reverse lookup of address/UTXO → `.zec` name)
* dApps and marketplaces (profile names, artist handles, DAO namespaces, etc.)

---

## 7. Reference Implementation & Ecosystem

### 7.1 Zord: Rust Indexer & Explorer

The **`zord`** repository is the reference ZORD implementation. It is a Rust indexer with built‑in explorer and APIs.([GitHub][5])

Key features:

* Connects to a Zcash full node via JSON‑RPC.
* Replays the entire transparent chain, assigning ordinal indices to zats.
* Extracts inscriptions and applies ZRC‑20, ZRC‑721, and ZNS rules.
* Serves a web UI and JSON REST APIs.

From the README:([GitHub][5])

> “Zord is a Rust indexer for Zcash ordinals (“Zats”) with first-class support for ZRC-20 tokens and ZNS names. The project follows Casey Rodarmor’s `ord` conventions, is released under CC0-1.0, and ships an opinionated web UI plus documented APIs so explorers can track inscriptions in real time.”

APIs include:

* `/api/v1/inscriptions`
* `/api/v1/tokens`
* `/api/v1/names`
* Ord‑compatible routes: `/inscription/:id`, `/content/:id`, `/preview/:id`

These endpoints power explorers, dashboards, and marketplaces.

**Quick start (from repo):**

```bash
# native
cargo run --release

# docker compose
API_PORT=3333 docker compose up -d
```

You must configure your Zcash RPC credentials (see `.env.example` in the repo).

### 7.2 Zatoshi RPC Node

`https://rpc.zatoshi.market/` is a public Zcash RPC endpoint tuned for ZORD workloads (block streaming, transaction queries, etc.).([rpc.zatoshi.market][8])

In production, operators should run their own hardened `zcashd`/`zebra` nodes and point Zord at them.

### 7.3 Zatoshi Mempool

`https://mempool.zatoshi.market/` provides a Zcash mempool explorer, analogous to `mempool.space` for Bitcoin, but **ZORD‑aware**: it surfaces unconfirmed inscriptions, ZRC‑20 mints, transfers, and NFT activity alongside standard network metrics.

### 7.4 Zord Explorer

Zord’s web UI (running e.g. at `http://135.181.6.234:3333/` in the current deployment) lets users:

* Browse latest inscriptions, collections, and ZRC‑20 tokens.
* Drill into specific inscription IDs and view decoded JSON.
* Explore ZNS names and ownership.

It relies entirely on Zord’s HTTP APIs; any third‑party can run their own instance.

### 7.5 Zatoshi Market

`https://www.zatoshi.market/` is the flagship **ZORD‑aware marketplace and launchpad**:([Zatoshi][9])

* **Inscribe** new assets on Zcash.
* **Mint and trade ZRC‑20** tokens (with live mint view and trending tokens).
* **Mint and trade ZRC‑721** NFT collections.
* Integrates tightly with Zatoshi Wallet for a seamless UX.

The homepage describes it as a “ZCASH INSCRIPTION MARKETPLACE” and exposes a dedicated **ZRC‑20 market** with trending tokens and live mint feeds.

### 7.6 Zatoshi Wallet

**Zatoshi Wallet** is the primary **ZORD‑aware Zcash wallet**:

* In‑browser, **client‑side** wallet (not a browser extension yet).
* Keys are generated locally, stored in the browser, and encrypted at rest with the user’s password.
* Supports:

  * Native ZEC
  * Viewing and managing **ZRC‑20 balances**
  * Viewing and managing **ZRC‑721 NFTs**
  * Connecting to Zatoshi Market for minting & trading
  * Creating new inscriptions (deploy/mint/transfer, NFTs, ZNS)

Roadmap (as described):

* Browser extension version.
* iOS and other mobile clients.

Security model:

* All signing happens client‑side; RPC nodes and markets never see private keys.
* Password‑protected key storage mitigates casual device compromise but should be combined with standard user opsec (strong passwords, hardware wallet integration once available, etc.).

---

## 8. Privacy Model

### 8.1 Does ZORD “break” Zcash privacy?

No. ZORD operates entirely on:

* Zcash’s **existing transparent layer** (P2PKH/P2SH outputs), and
* Lightweight text inscriptions embedded in that layer.

Zcash’s shielded pools and privacy protocols remain completely unchanged.([Coinpare][4])

**Key points:**

* **No consensus changes**: ZORD does not alter the protocol; it only interprets data that Zcash already allows (e.g., OP_RETURN/script data).
* **No new global linkage**: The act of tracking individual zats is purely off‑chain logic. Zcash nodes are unaware of ordinals or inscriptions.

### 8.2 What privacy do ZORD assets have?

Current ZORD activity uses **transparent transactions**, so:

* Anyone can see that some transparent address holds a given ZRC‑20 balance or NFT.
* However, as with Bitcoin, that address is just a pseudonym unless you leak it.

Compared to other chains:

* **Bitcoin ordinals & Solana NFTs**:
  All transfers and holdings are permanently public on transparent ledgers. There is no first‑class notion of shielded value; addresses are reused across DeFi, CEX withdrawals, and token trading, making deanonymization common.
* **ZORD on Zcash**:

  * Users can keep their *ZORD‑facing* transparent addresses logically separate from any identity‑linked activity.
  * Users can fund those addresses from shielded pools, so inbound ZEC flows are shielded and unlinkable, then “de‑shielded” only to the extent necessary to inscribe or trade.
  * Outbound trading can likewise be followed by re‑shielding, keeping long‑term holdings in shielded form.

Thus, **in practice**:

> Owning and trading NFTs or memecoins via ZORD on Zcash can be *significantly more private* than equivalent activity on Solana or Bitcoin ordinals, **provided** users:
>
> * Avoid linking their transparent addresses to KYCed identities.
> * Use shielded pools for funding, withdrawal, and long‑term storage.
> * Rotate transparent addresses regularly.

### 8.3 Limitations

* **Transparent inscriptions are still visible**:
  Anyone can see that some address minted a specific ZRC‑20 or NFT and track its transfers within the transparent layer.
* **No current shielded inscription standard**:
  ZORD does not yet define inscriptions inside shielded memos. Doing so would require careful design to preserve shielded privacy semantics while still allowing indexers to interpret data (likely via selective disclosure or view keys).

### 8.4 Future directions

Potential privacy‑enhancing extensions:

* **Shielded‑friendly ZRC standards**:

  * Embed inscription envelopes in shielded memos with explicit opt‑in viewing keys.
  * Let users hold and trade tokens entirely within shielded pools while revealing only what marketplaces need to know.
* **Unlinkable address tooling**:

  * Wallet features for automatic address rotation.
  * Integration with new Zcash wallet UX improvements that encourage shielding by default.([Galaxy][10])

---

## 9. Security and Trust Model

### 9.1 Consensus vs metaprotocol

* Zcash consensus defines what is a valid block and transaction.
* ZORD defines how to **interpret** some of that data as inscriptions and tokens.

Security properties:

* A malicious indexer cannot create tokens out of thin air *without corresponding valid inscriptions on-chain*.
* Different indexers will converge on the same state if they:

  * Reindex the same chain; and
  * Follow the canonical rules (first valid deploy wins, supply caps, etc.) for ZRC‑20/ZRC‑721/ZNS.

### 9.2 Reorgs and forks

* As with ordinals on Bitcoin, ZORD must handle **chain reorgs**:

  * If a block with inscriptions is reorged out, the inscriptions disappear from canonical history.
  * Indexers must roll back state and re‑apply inscriptions from the new main chain.

### 9.3 Validation rules

Implementations should:

* Strictly validate JSON envelopes:

  * Enforce correct types and required fields.
  * Reject malformed JSON or invalid numeric strings.
* Enforce all supply constraints:

  * ZRC‑20 `max` and per‑mint `lim`.
  * ZRC‑721 `supply` and unique `id`.
* Make indexer logic **deterministic**:

  * No time‑based or external‑state decisions.
  * Canonical chain view only.

### 9.4 Wallet and marketplace trust

* Zatoshi Wallet is non‑custodial but users still trust it to:

  * Display correct balances and inscription metadata.
  * Construct and sign transactions according to their intent.
* Marketplaces like Zatoshi Market must:

  * Use consistent ZORD indexers (or their own compatible implementation).
  * Clearly surface fees and royalty behavior.
  * Never take custody of user keys.

---

## 10. Running ZORD

High‑level recipe:

1. **Run a Zcash full node**

   * `zcashd` or `zebra`, fully synced.
   * Expose JSON‑RPC to Zord.

2. **Configure Zord**

   * Clone `https://github.com/zatoshilabs/zord` and set env vars:

     * `ZCASH_RPC_URL`, `ZCASH_RPC_USER`, `ZCASH_RPC_PASSWORD`
     * Optional: `VERBOSE_LOGS=true` to bump logging.([GitHub][5])

3. **Index the chain**

   * Run `cargo run --release` or docker compose.
   * Zord will ingest blocks, construct the ordinal index, and populate inscription/token/name tables.

4. **Expose the explorer**

   * Point a reverse proxy (nginx, etc.) at Zord’s HTTP server.
   * Optionally deploy the UI at a public hostname.

5. **Integrate wallets/markets**

   * Call `/api/v1/inscriptions`, `/api/v1/tokens`, `/api/v1/names`, etc.
   * Use ord‑style routes for compatible tooling.

---

## 11. Summary

ZORD brings **ordinal theory** to **Zcash**:

* The **ZORD metaprotocol** serializes zats and binds arbitrary data to them.
* **ZRC‑20** and **ZRC‑721** provide minimal, JSON‑only standards for fungible and non‑fungible assets on Zcash.([zatoshi.gitbook.io][1])
* **ZNS** introduces `.zec` / `.zcash` namespaces based on first‑inscription‑wins semantics.
* The **Zord indexer**, **Zatoshi RPC**, **Zatoshi mempool**, **Zatoshi Market**, and **Zatoshi Wallet** form a coherent ecosystem for minting, trading, and holding NFTs and memecoins on Zcash.

Crucially, ZORD:

* **Does not modify** Zcash consensus.
* **Does not conflict** with Zcash’s privacy mission; instead, it provides a *more private* foundation for NFTs and tokens than transparent‑only L1s, as long as users respect basic privacy hygiene.

You can think of ZORD as:

> *Ordinals reimagined for Zcash – same ordinal magic, new privacy‑aware home.*

If you’d like, I can next help you extract sections of this into separate `README`s for the `zord`, `zrc`, and `wallet` repos, or draft a short “marketing” version for non‑technical users.

[1]: https://zatoshi.gitbook.io/zrc "ZRC-20: Zcash Fungible Token Standard | zrc"
[2]: https://zatoshi.gitbook.io/zrc/721 "ZRC-721: Zcash NFT Inscription Standard | zrc"
[3]: https://github.com/ordinals/ord?utm_source=chatgpt.com "ordinals/ord: 👁‍🗨 Rare and exotic sats"
[4]: https://www.coinpare.io/whitepaper/zcash.pdf?utm_source=chatgpt.com "Zcash Protocol Specification"
[5]: https://github.com/zatoshilabs/zord "GitHub - zatoshilabs/zord:  rare and exotic zats"
[6]: https://forum.zcashcommunity.com/t/transparent-shielded-dex-with-maya-protocol/46857?page=2&utm_source=chatgpt.com "Transparent & Shielded DEX with Maya Protocol - Page 2"
[7]: https://en.wikipedia.org/wiki/Zcash?utm_source=chatgpt.com "Zcash"
[8]: https://rpc.zatoshi.market/ "Zcash RPC Node | zatoshi.market"
[9]: https://www.zatoshi.market/ "zatoshi.market"
[10]: https://www.galaxy.com/insights/research/zcash-price-zec-near-intents-zashi-wallet-privacy-zero-knowledge-proofs?utm_source=chatgpt.com "Why Has Zcash Suddenly Soared? - Galaxy"
