# Self-Hosting `fern-server`

`fern-server` is Fern's optional, lightweight sync server. It stores the latest synced change for each row in a single SQLite database (`fern-server.db`) and keeps attached files and countdown backgrounds in a sibling `<data>.files/` directory.

This guide covers the security model, TLS reverse-proxy deployment (Caddy and Nginx), Docker & Docker Compose, Linux `systemd`, safe online backups, and attachment garbage collection.

---

## Security Model & End-to-End Encryption (E2EE)

### End-to-End Encryption (Optional, Recommended)

When you set an **Encryption Passphrase** in **Settings → Data & Sync** on your devices, Fern encrypts synced payloads client-side before sending them to `fern-server`:

- **Key derivation:** **PBKDF2-HMAC-SHA256** (200,000 iterations, domain salt `"fern-e2ee-v1"`) derives a 256-bit symmetric key from your passphrase.
- **Authenticated encryption:** **ChaCha20-Poly1305** with a fresh random 12-byte nonce per encryption.
- **Database rows:** Each synced row's JSON content (task titles, notes, comments, list/habit/countdown names, tags, schedules, etc.) is encrypted into an envelope `{"__fern_e2ee": 1, "nonce": "…", "ct": "…"}` bound with domain-separated AAD (`fern-row-v1:<entity>:<key>`), preventing ciphertext from being swapped across tables or rows.
- **Attachments & countdown backgrounds:** Each uploaded file is encrypted as `FERNE2EE\x01 || 12-byte nonce || ciphertext || 16-byte Poly1305 tag` (+37 bytes overhead) bound with AAD (`fern-file-v1:<filename>`).

> **Keep your passphrase safe.** Your passphrase never leaves your devices and cannot be recovered from `fern-server`. Every device syncing to the server must use the exact same passphrase.

### What Metadata the Server Still Sees

Even with E2EE enabled, `fern-server` must inspect minimal structural metadata to relay changes and garbage-collect deleted files:

- Entity (table) names (for example `tasks`, `lists`, `task_attachments`, `countdowns`)
- Row keys (UUIDs, or composite keys such as `task_id`/`tag`)
- Change timestamps (`at`, Unix milliseconds) and server sequence numbers (`seq`)
- Syncing device IDs (`device`, UUID v7)
- Storage file keys (`file_key`) for attachments and countdown covers, so server-side garbage collection can preserve live encrypted files and reclaim orphaned ones
- Approximate row payload sizes, attachment file sizes, sync frequency, and client IP addresses

### Why TLS (HTTPS) Is Still Mandatory

`fern-server` intentionally speaks plain HTTP on a local or internal interface. **Always terminate TLS in front of `fern-server`** when exposing it over any untrusted network, even if E2EE is enabled:

- Every authenticated request carries `Authorization: Bearer <FERN_TOKEN>`. Without TLS, anyone on the network can capture the bearer token and overwrite, corrupt, or delete synced state, or fill the server's disk.
- TLS protects transport metadata, prevents active man-in-the-middle tampering, and authenticates your server's identity.

---

## Configuration Reference

`fern-server` reads configuration from CLI flags with environment-variable fallbacks (CLI flags take precedence):

| CLI Flag | Environment Variable | Default | Description |
| --- | --- | --- | --- |
| — | `FERN_TOKEN` | *(required to serve)* | Shared bearer token (minimum 16 characters). Generate with `openssl rand -base64 32`. |
| `--listen <ADDR>` | `FERN_LISTEN` | `127.0.0.1:8650` | Bind address and port. |
| `--data <PATH>` | `FERN_DATA` | `fern-server.db` | SQLite database path; files are stored alongside it in `<PATH>.files/`. |
| `--gc-grace-hours <HOURS>` | `FERN_GC_GRACE_HOURS` | `24` | Grace period before unreferenced files in `<PATH>.files/` are eligible for deletion. |
| `--gc-now` | — | *(off)* | Run a single garbage-collection pass, print JSON stats to stdout, and exit (does not require `FERN_TOKEN`). |

### HTTP Endpoints & Protocol Versioning

- `GET /health` *(unauthenticated)* — Returns HTTP `200` with:
  ```json
  { "status": "ok", "version": "0.2.0", "protocol": 2, "min_protocol": 1 }
  ```
- `POST /sync` *(Bearer auth)* — Exchanges row changes and returns `"protocol"` and `"min_protocol"`. Clients speaking an unsupported protocol version receive HTTP `426 Upgrade Required` with:
  ```json
  { "error": "incompatible_protocol", "server_protocol": 2, "min_protocol": 1, "client_protocol": 99 }
  ```
- `PUT /files/<key>`, `GET /files/<key>`, `DELETE /files/<key>` *(Bearer auth)* — Stores, fetches, or deletes individual attachment and countdown-cover blobs.
- `POST /gc` *(Bearer auth)* — Triggers an immediate `.files` garbage-collection pass with optional JSON body `{"min_age_secs": 3600}` (defaults to the server's `--gc-grace-hours` setting).

On startup, `fern-server` runs `PRAGMA integrity_check(10)` and refuses to start if the SQLite database is corrupt.

---

## Deploying Behind TLS

Bind `fern-server` to `127.0.0.1:8650` (or a private Docker network) and place Caddy or Nginx in front of it.

### Caddy (Automatic Let's Encrypt)

Caddy automatically obtains and renews TLS certificates via Let's Encrypt and redirects HTTP to HTTPS.

Create `/etc/caddy/Caddyfile`:

```caddyfile
sync.example.com {
    reverse_proxy 127.0.0.1:8650

    header {
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        X-Content-Type-Options "nosniff"
        Referrer-Policy "no-referrer"
    }
}
```

Reload Caddy:

```bash
sudo systemctl reload caddy
```

### Nginx (with Let's Encrypt / Certbot)

Nginx defaults to a 1 MiB body limit, which will reject larger sync payloads and attachments. Set `client_max_body_size 32M;` (or `33M;` if you upload maximum-size 32 MiB attachments with E2EE enabled, to accommodate the 37-byte encryption envelope):

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name sync.example.com;

    location / {
        return 301 https://$host$request_uri;
    }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name sync.example.com;

    ssl_certificate     /etc/letsencrypt/live/sync.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/sync.example.com/privkey.pem;
    ssl_protocols       TLSv1.2 TLSv1.3;
    ssl_session_timeout 1d;
    ssl_session_cache   shared:FernTLS:10m;

    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options "nosniff" always;

    # Match Fern's 32 MiB attachment & sync payload limit (use 33M if uploading 32 MiB E2EE files).
    client_max_body_size 32M;

    location / {
        proxy_pass http://127.0.0.1:8650;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;
    }
}
```

Test and reload Nginx:

```bash
sudo nginx -t && sudo systemctl reload nginx
```

---

## Docker & Docker Compose

### Production Stack with Docker Compose (Recommended)

The repository ships a complete production stack in [`deploy/docker-compose.yml`](../deploy/docker-compose.yml) combining `fern-server` (bound strictly to an internal Docker network with a read-only root filesystem) and `caddy:2-alpine` for automatic HTTPS.

1. Point your domain's DNS `A`/`AAAA` record (for example `sync.example.com`) at your server and ensure ports `80` and `443` are reachable.
2. Create `deploy/.env` with strict permissions:
   ```bash
   cat <<EOF > deploy/.env
   FERN_DOMAIN=sync.example.com
   FERN_TOKEN=$(openssl rand -base64 32)
   FERN_GC_GRACE_HOURS=24
   EOF
   chmod 600 deploy/.env
   ```
3. Build and start the stack:
   ```bash
   docker compose -f deploy/docker-compose.yml up -d --build
   ```
4. Verify the server is healthy:
   ```bash
   curl -fsS https://sync.example.com/health
   ```

### Standalone Docker Container

To build and run only `fern-server` behind an existing reverse proxy on the host:

```bash
docker build -t fern-server .
docker run -d \
  --name fern-server \
  --restart unless-stopped \
  -p 127.0.0.1:8650:8650 \
  -v fern_data:/data \
  -e FERN_TOKEN="$(openssl rand -base64 32)" \
  fern-server
```

The image runs as the unprivileged `fern` user, persists data in `/data`, defaults to `FERN_LISTEN=0.0.0.0:8650` and `FERN_DATA=/data/fern-server.db`, and defines a `HEALTHCHECK` against `http://127.0.0.1:8650/health`.

---

## Linux `systemd` Deployment

1. Install the `fern-server` binary (from a release archive or `cargo build --release --locked -p fern_server`):
   ```bash
   sudo install -m 0755 target/release/fern-server /usr/local/bin/fern-server
   ```
2. Create `/etc/fern-server.env` readable only by `root` (systemd reads `EnvironmentFile` before dropping privileges for `DynamicUser=yes`):
   ```bash
   sudo install -m 0600 -o root -g root /dev/null /etc/fern-server.env
   printf 'FERN_TOKEN=%s\nFERN_GC_GRACE_HOURS=24\n' "$(openssl rand -base64 32)" | sudo tee /etc/fern-server.env >/dev/null
   ```
3. Install the hardened unit file from [`deploy/fern-server.service`](../deploy/fern-server.service):
   ```bash
   sudo cp deploy/fern-server.service /etc/systemd/system/fern-server.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now fern-server
   ```
4. Verify status and health:
   ```bash
   sudo systemctl status fern-server
   curl -fsS http://127.0.0.1:8650/health
   journalctl -u fern-server -f
   ```
5. Put Caddy or Nginx in front of `http://127.0.0.1:8650` as shown above.

---

## Backups & Restore

`fern-server` opens its SQLite database in WAL mode (`PRAGMA journal_mode = WAL`). **Do not copy `fern-server.db` with `cp` while the server is running**, as a raw file copy can miss uncommitted WAL frames and produce a corrupt backup.

### Safe Online Backup (`systemd` / Host)

Use SQLite's online backup command (`sqlite3 ... ".backup ..."`) together with `rsync` for the `.files` directory:

```bash
BACKUP_DIR="/var/backups/fern/$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$BACKUP_DIR"

# 1. Consistent online snapshot of the SQLite database
sudo sqlite3 /var/lib/fern-server/fern-server.db ".backup '$BACKUP_DIR/fern-server.db'"

# 2. Mirror attachment and countdown-cover blobs
sudo rsync -a /var/lib/fern-server/fern-server.db.files/ "$BACKUP_DIR/fern-server.db.files/"

# Optional: verify the snapshot's integrity
sqlite3 "$BACKUP_DIR/fern-server.db" "PRAGMA integrity_check;"
```

### Safe Backup (Docker Compose)

For a Docker Compose volume (`fern_data`), briefly stop `fern-server` (or run `sqlite3 .backup` in a helper container mounting `fern_data`) and archive `/data`:

```bash
docker compose -f deploy/docker-compose.yml stop fern-server
docker run --rm \
  -v deploy_fern_data:/data:ro \
  -v "$PWD/backups:/backup" \
  debian:bookworm-slim \
  tar -czf "/backup/fern-server-$(date -u +%Y%m%dT%H%M%SZ).tar.gz" -C /data .
docker compose -f deploy/docker-compose.yml start fern-server
```

*(Confirm the exact volume name with `docker volume ls`.)*

### Restoring a Backup

1. Stop `fern-server`.
2. Replace `fern-server.db` and `fern-server.db.files/`, and remove any stale `fern-server.db-wal` and `fern-server.db-shm` files from the old instance.
3. Ensure ownership matches the service user (`fern:fern` in Docker; `StateDirectory=fern-server` under systemd).
4. Start `fern-server` and verify `GET /health` returns HTTP `200`.

---

## `.files` Garbage Collection

When tasks, attachments, or countdown background images are deleted or replaced, clients remove their blobs proactively via `DELETE /files/<key>`. To reclaim any orphaned blobs left behind by interrupted uploads, offline deletions, or older clients, `fern-server` also provides built-in garbage collection.

### How It Works

1. `fern-server` collects every live `file_key` stored in the `changes` table (including both plaintext and E2EE attachment/countdown-cover entries, plus legacy plaintext rows from upgraded v0.2.0 databases).
2. It scans `<data>.files/` for regular files not present in the live key set.
3. Any unreferenced file newer than the configured **grace period** (default **24 hours**) is skipped so in-flight uploads that have not yet committed their metadata change are never removed.
4. Unreferenced files older than the grace period are deleted and reported in the GC stats (`scanned`, `removed`, `freed_bytes`, `skipped_recent`).

### Automatic Periodic GC

While serving, `fern-server` automatically runs a GC pass every **6 hours** or every **200 sync requests** using the configured grace period (`--gc-grace-hours` / `FERN_GC_GRACE_HOURS`, default `24`). Removed file counts are logged to stderr whenever one or more files are reclaimed.

### On-Demand GC via HTTP (`POST /gc`)

Trigger a pass immediately against a running server:

```bash
# Using the server's default grace period
curl -fsS -X POST http://127.0.0.1:8650/gc \
  -H "Authorization: Bearer $FERN_TOKEN"

# Override the grace period (e.g. 1 hour = 3600 seconds)
curl -fsS -X POST http://127.0.0.1:8650/gc \
  -H "Authorization: Bearer $FERN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"min_age_secs": 3600}'
```

Example response:

```json
{"freed_bytes":4194304,"removed":2,"scanned":18,"skipped_recent":1}
```

### Offline / Scheduled GC via CLI (`--gc-now`)

For cron jobs or maintenance scripts when you want to run a single pass directly against the database without starting the HTTP listener:

```bash
fern-server --data /path/to/fern-server.db --gc-grace-hours 24 --gc-now
```

This prints the JSON stats object to stdout and exits with status `0`.

> **Note for `systemd` (`DynamicUser=yes`):** Prefer `POST /gc` against `http://127.0.0.1:8650/gc` while `fern-server.service` is running so you do not create root-owned SQLite WAL files inside `/var/lib/fern-server`.
