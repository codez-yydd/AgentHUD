# zbar-sync · ZBar Multi-Device Sync Server

The ZBar client uploads local ZCode usage data incrementally to this service, enabling aggregated viewing across multiple machines (office / home).

**Self-hosted, single-user**: you deploy it yourself, data stays on your own server. Supports HTTP / HTTPS / bare IP.

Tech stack: **Python 3 + Flask + stdlib sqlite3** — zero compilation, runs immediately.

---

## Quick Start

### 1. Upload files to your server

Upload **these files** from the `server/` folder (do NOT upload `.venv/` or `zbar-data/`):

```
app.py          # Main app (all endpoints)
db.py           # Database ops (auto-creates DB & tables)
auth.py         # Auth (master token / device token)
config.py       # Config (port, data dir)
start.sh        # Launch script
requirements.txt
```

### 2. Install dependencies

**Run in the terminal** (avoid the BT Panel "Python Project Manager" — it may throw pip errors):

```bash
cd /your/upload/path/zbar-sync
pip3 install flask
```

> Only Flask is needed. Python 3.8+ required; sqlite3 is part of the standard library.
> If `pip3` errors out, fix it first: `python3 -m ensurepip --upgrade`, or use `python3 -m pip install flask`.

### 3. Start

```bash
./start.sh
```

Or directly:

```bash
python3 app.py
```

On first launch it automatically:
- Creates the `zbar-data/` directory
- Creates the SQLite database (`zbar-data/usage.db`)
- Generates a Master Token and prints it to the log

Output looks like:

```
[zbar-sync] Initialized
[zbar-sync] MASTER_TOKEN: 9f3a7c2e8b1d4a6f...
[zbar-sync]   ↑ Copy this token to the client "Sync Settings" to register a device
[zbar-sync] Listening on port: 3838
```

**Copy this Master Token** (you can always view it later with `cat zbar-data/master.token`).

### 4. Open the port

Open port **3838** (TCP) in your firewall / security group.
Cloud providers (AWS / Azure / etc.) also need the port opened in their **security group** settings.

### 5. Connect the client

ZBar panel → click **⇅** → fill in:
- Server URL: `http://your-server-ip:3838`
- Credential: paste the Master Token
- Device name: e.g. `work` / `home`

Click "Connect & Register" to finish.

---

## Keep it running (process supervisor)

The process exits when you close the terminal. Use a process manager to keep it alive:

**systemd (Linux):**

```bash
cat > /etc/systemd/system/zbar-sync.service <<'EOF'
[Unit]
Description=ZBar Sync Server
After=network.target

[Service]
Type=simple
WorkingDirectory=/your/upload/path/zbar-sync
ExecStart=/usr/bin/python3 /your/upload/path/zbar-sync/app.py
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl enable --now zbar-sync
```

**BT Panel**: install the "Process Supervisor Manager" plugin → add a supervisor process:
- Name: `zbar-sync`
- User: `root`
- Working dir: `/your/upload/path/zbar-sync`
- Start command: `python3 /your/upload/path/zbar-sync/app.py`

---

## Custom port

Set the `PORT` environment variable at startup:

```bash
PORT=8080 python3 app.py
```

---

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3838` | Listen port |
| `DATA_DIR` | `./zbar-data` | Data directory (DB + master.token) |
| `HOST` | `0.0.0.0` | Listen address (0.0.0.0 = accessible externally) |

---

## API Overview

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `POST /register` | Master Token | Register a device, returns Device Token |
| `POST /sync` | Device Token | Incremental upload of usage records (each record may carry source, defaults to zcode) |
| `GET /usage` | Device Token | Aggregated query (overall + by_model + trend, optional source filter) |
| `GET /usage_records` | Device Token | Per-record usage query (total + rows + by_model; powers the request-logs page "all devices" scope) |
| `GET /models` | Device Token | Distinct model list across all devices × sources (for pricing setup) |
| `POST /pricing` | Device Token | Desktop publishes per-model price changes (merged by revision) |
| `GET /pricing` | Device Token | Desktops/mobile read the global price table (with revision) |
| `GET /devices` | Device Token | Device list |
| `POST /device/revoke` | Master Token | Revoke a device |
| `POST /cleanup` | Master Token | Data cleanup (by device / by time / all / unattributed snapshots / reset) |
| `GET /cleanup/status` | Device Token | Data volume + auto-cleanup config |
| `POST /cleanup/config` | Master Token | Configure scheduled auto-cleanup |
| `GET /health` | None | Health check |

---

## Codex / Claude Data (source dimension)

Newer clients also upload usage records for Codex CLI, Claude Code, Kimi Code, Cursor, DeepSeek Harness, OpenCode and MiniMax Code in addition to ZCode usage. Sources are distinguished by the `source` field: the same device and the same `local_rowid` never conflict across sources (each source has its own independent rowid sequence), and their upload/query cursors are independent as well.

**Upload whitelist**: the schema itself does not constrain source values, but uploads are gated by a whitelist (`SYNC_SOURCE_WHITELIST` in `app.py`, currently `zcode` / `codex` / `claude` / `kimi` / `cursor` / `dsh` / `opencode` / `minimaxcode`); a batch whose source is outside it is rejected with HTTP 400. **Adding a source on the client therefore requires adding it to the whitelist too** — otherwise that source keeps failing with 400 and its cursor never advances (no data is lost: it is uploaded automatically once the server is upgraded).

**Upgrade**: pull the new code and restart the service. On first launch the `usage_records` table structure is migrated automatically (a `source` column is added, primary key becomes `(device_id, source, local_rowid)`); all existing data is kept intact and marked as `zcode`, and indexes are rebuilt. Upgrading the server before the clients is recommended.

**Upgrade-order protection**: before uploading Codex / Claude data, new clients probe the server protocol version (the `/sync` response now includes a `proto: 2` field). Old servers do not return it, so the client neither uploads such data nor advances the cursor (the sync log shows "server version too old"); after the server is upgraded, uploading resumes automatically — no data is lost even if clients are upgraded first. Old clients are unaffected.

**API changes** (all backward compatible):

- `POST /sync`: each record gains a `source` field, defaulting to `zcode` (old clients that omit it are treated as zcode). Each batch contains a single source; `last_rowid` / `max_rowid` count within that source's own rowid sequence. Re-sending the same primary key with a larger `computed_total_tokens` overwrites the old row (Claude Code sessions stream to disk, so the client re-uploads final values to correct previously uploaded intermediate ones; zcode/codex records are immutable and re-sends are no-ops due to the overwrite guard).
- `GET /usage`: new optional query parameter `source` (`zcode` / `codex` / `claude`); omitting it merges all sources. Every group in `by_model` and `trend.by_model` gains a `source` field for frontend display.
- `POST /period_detail`: the body also accepts an optional `source` field.
- `GET /models`: returns the distinct model list across all devices and sources (`[{source, provider_id, model_id}]`). The client's pricing page uses it to show models that exist only on other devices and to include them in pricing-update checks. Against an older server without this endpoint the client silently falls back to local lists only; upgrade the server to enable it.

---

## proto 6: per-record usage query (GET /usage_records)

New read-only endpoint, no schema changes involved: the request-logs page's "all devices" scope uses it to fetch per-request usage details from other devices. Query parameters: `from_ms` / `to_ms` (required, half-open interval), `source` / `model_id` (optional filters), `devices` / `exclude_device` (device-set filter, same semantics as `/usage`), `limit` / `offset` (row pagination, limit clamped to 5000). Response `{total, rows, by_model}`: `rows` returns full per-record fields (with `device_id`) ordered by `started_at DESC` (stable tie-break on `device_id/source/local_rowid` within the same millisecond); `by_model` is grouped by `(source, provider_id, model_id)` so the client can fold remote totals into the matching source lane for cost conversion. Remote queries always carry `exclude_device=<this device>`, so the remote set is disjoint from locally read data and concatenation needs no dedup. Against an old server without this endpoint (404) or during a hot-swap window (new `app.py` + old `db.py`, empty result), the client automatically degrades to local-only scope without errors.

---

## Pricing Sync (POST / GET /pricing)

Mobile no longer edits model prices; the desktop is the only editor. The server stores the **single global** price table, and both desktops and mobile treat it as the source of truth. Desktops push per-model deltas with `base_revision`; the server merges and increments revision; every desktop pulls the full table on its next sync, while mobile stays read-only.

```http
POST /pricing
Authorization: Bearer <device_token>
{
  "base_revision": 3,
  "changes": {
    "glm-4.6": {"input":1.39,"output":4.17,"cache_read":0.14},
    "old-model": null
  },
  "hidden": ["old-model"],
  "fx_rate": 7.2
}
→ {"revision":4,"updated_at":...,"updated_by":"device-a","usd":{...},"hidden":[...],"fx_rate":7.2,"synced":true,"applied":2,"concurrent":false}

GET /pricing
Authorization: Bearer <device_token>
→ the same table (revision / usd / hidden / fx_rate)
```

- **Per-model merge**: different devices editing different models never overwrite each other; for the same model the last write accepted by the server wins, and `concurrent` lets the client surface a "merged with another device" notice;
- **First run / empty server**: the desktop publishes its full local table as the initial version; if the server already has a table, the server table wins and the desktop aligns to it;
- **Works without a server**: when sync is not configured the desktop only reads/writes local `pricing.json` and makes no network calls;
- **Offline edits**: pending changes stay in `pricing_sync.json` and retry on the next sync; local billing is unaffected;
- **Revision & validation**: `revision` increments server-side and `updated_at` uses the server clock; invalid prices/rates are rejected with 400. Deleting every model is allowed (an empty table means zero cost);
- **No schema change**: the JSON lives in the `config` table (`key=pricing`); upgrade by replacing `app.py` and restarting;
- **Compatibility**: the legacy full-table `usd` payload is still accepted as a full replace; against an older server without `/pricing`, the desktop retries silently (usage sync is unaffected) and mobile falls back to its cached table or local reference prices. Old clients never call this endpoint.

---

## Data Cleanup

Operate via the client "Sync Settings → Data Management", or call the API directly.

| Action | Description | Irreversible |
|--------|-------------|--------------|
| By device | Delete all records of a device | ✅ |
| By time | Delete data older than N days (shortens trend history) | ✅ |
| All | Clear all usage data, keep device registrations | ✅ |
| Reset | Clear everything including devices, back to initial state | ✅ |

> **Cleanup is deletion**: cleaned data is NOT re-uploaded from the client.

---

## Data Storage

```
zbar-data/
├── master.token     # Master Token (persisted, not regenerated on restart)
├── usage.db         # SQLite database (auto-created)
└── usage.db-wal     # WAL log (managed by SQLite)
```

Backup: just copy the `zbar-data/` directory.

---

## Security Notes

- **HTTP warning**: when using HTTP or bare IP, tokens and usage data travel in plaintext over the network. Recommended for intranet use, or set up HTTPS via an Nginx reverse proxy.
- **Auth**: Master Token for registration admission, Device Token for daily auth (server stores only hashes).
- **Privacy**: synced data contains only model names, token counts, and timestamps — **no code or conversation content**.
