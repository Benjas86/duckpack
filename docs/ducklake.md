[← Docs Index](./index.md)

# DuckLake Lakehouse Guide

DuckPack treats **DuckLake** as a first-class lakehouse catalog alongside native DuckDB: supporting all four metadata backends, remote/frozen DuckLake URLs, metadata-only schema evolution, physical rewrite safety guards, automated commit-message tagging, and interactive snapshot time travel.

---

## The DuckLake Connection Triad

Every DuckLake catalog combines three components: a **Metadata Catalog**, a **Parquet Storage `DATA_PATH`**, and optional **Storage/Catalog Secrets**.

```mermaid
flowchart LR
    DP["DuckPack Session"]
    META["Metadata Catalog\n(DuckDB File | SQLite | PostgreSQL | Quack)"]
    STORE["Parquet Storage DATA_PATH\n(Local Dir | S3 | R2 | GCS | Azure | HTTPS)"]
    SEC["Scoped Secrets\n(TYPE S3 / R2 / GCS / AZURE / QUACK)"]

    DP -->|"ATTACH ducklake:..."| META
    DP -->|"Reads & Writes Parquet"| STORE
    SEC -.->|"Authenticates"| META
    SEC -.->|"Authenticates"| STORE
```

---

## Connecting to DuckLake

### 1. Declarative `duckpack.toml` (`[[attach]]` or `[env.<name>]`)

#### DuckLake on Quack + MinIO / S3 (Multi-Container Pattern)
```toml
[[secrets]]
name = "lake_s3"
type = "s3"
scope = "s3://warehouse/"
endpoint_env = "S3_ENDPOINT"
key_id_env = "S3_ACCESS_KEY"
secret_env = "S3_SECRET_KEY"
url_style = "path"
use_ssl = false

[[attach]]
alias = "sales_lake"
type = "ducklake"
backend = "quack"
metadata = "quack:sales-meta.internal:9494"
data_path = "s3://warehouse/sales/"
token_env = "QUACK_SALES_TOKEN"
client_id = "analytics_admin"
insecure = true
default = true
```

> **Per-Alias Credential & RBAC Isolation:** DuckPack attaches DuckLake-on-Quack catalogs using DuckLake's native typed `META_DISABLE_SSL`, `META_TOKEN`, and `META_CLIENT_ID` parameters. This lets you mount multiple independent DuckLake-on-Quack containers — or distinct RBAC roles (`admin`, `writer`, `reader`) against the same DuckLake server — in the same session with zero global secret or client-ID collisions. See [Quack RBAC `ducklake_mode`](./quack-and-rbac.md#3-ducklake-aware-quack-rbac-ducklake_mode--true).

#### DuckLake on PostgreSQL + AWS S3
```toml
[[attach]]
alias = "prod_lake"
type = "ducklake"
backend = "postgres"
host = "pg-meta.corp.internal"
port = 5432
database = "ducklake_catalog"
username = "ducklake_user"
password_env = "PG_LAKE_PASSWORD"
data_path = "s3://company-lakehouse/prod/"
```
*(You can also pass a full `postgresql://user:pass@host:5432/db` URI via `dsn_env` — DuckPack normalizes it to `ducklake:postgres:postgresql://...`.)*

#### DuckLake on Local DuckDB File or SQLite
```toml
[[attach]]
alias = "local_lake"
type = "ducklake"
backend = "file"                         # Or "sqlite"
path = "./metadata/warehouse.ducklake"
data_path = "./parquet_data/"            # Optional for DuckDB file (defaults to <file>.files)
```

#### Remote / Frozen DuckLake over HTTPS or S3
```toml
[[attach]]
alias = "tpch_sf1"
type = "ducklake"
backend = "file"
path = "https://blobs.duckdb.org/datalake/tpch-sf1.ducklake"
read_only = true
```
DuckPack detects remote URLs (`https://`, `http://`, `s3://`, `r2://`, `gcs://`, `az://`, etc.) and emits `ATTACH '<url>' AS "tpch_sf1" (TYPE ducklake, READ_ONLY);` (without a `ducklake:` prefix), matching the official DuckLake specification.

### 2. Interactive Attach Modal (`Ctrl+A`)

Press **`Ctrl+A`** inside `duckpack explore`, select **`DuckLake (Lakehouse)`**, and cycle the **Metadata Backend** (`DuckDB File` / `PostgreSQL` / `SQLite` / `Quack`):
- Supports pasting remote/frozen URLs (`https://...ducklake`, `s3://...ducklake`) directly into the Local File path field.
- Toggle **`[X] Save to duckpack.toml (Ctrl+S)`** to persist the connection into `duckpack.toml` and `.env`.

### 3. CLI Direct Target

```bash
# Using a named [env.lakehouse] profile from duckpack.toml
duckpack diff --env lakehouse
duckpack apply --env lakehouse

# Direct --db URI
duckpack explore --db "ducklake:metadata.ducklake (DATA_PATH './lake_data/')"
```

---

## Metadata-Only Schema Evolution & Physical Rewrite Guard

DuckLake supports zero-copy metadata-only `ALTER TABLE` operations (`ADD COLUMN`, `DROP COLUMN`, `RENAME COLUMN`, `RENAME TABLE`, lossless type promotions like `INT` → `BIGINT` or `FLOAT` → `DOUBLE`, `SET/DROP NOT NULL`).

### Constraint Handling on DuckLake
Per the DuckLake specification, DuckLake supports `NOT NULL` constraints only — `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, and `CHECK` are unsupported. When diffing a project against a DuckLake catalog, DuckPack automatically suppresses unsupported constraint diffs so your pipelines never report permanent false-positive drift.

### Protecting Against Expensive Parquet Rewrites (`--allow-lake-rewrite`)

If a migration requires a physical copy-and-swap table rewrite (`RecreateTable`), rewriting multi-terabyte Parquet datasets accidentally would be costly. DuckPack **blocks `RecreateTable` on DuckLake catalogs by default**:

- **CLI opt-in:**
  ```bash
  duckpack apply --env lakehouse --force-drop --allow-lake-rewrite --auto-approve
  ```
- **Environment variable (works across CLI and Python):**
  ```bash
  DUCKPACK_ALLOW_LAKE_REWRITE=true duckpack apply --env lakehouse --force-drop --auto-approve
  ```
- **In-IDE Diff Modal (`F6`):** Press **`w`** to toggle `Allow Lake Rewrite: [OFF/ON]` before pressing `Enter`.

---

## Automated Snapshot Commit Tagging

Every migration applied by DuckPack to a DuckLake catalog automatically tags the resulting DuckLake snapshot inside the transaction via `set_commit_message`:

```sql
CALL "sales_lake".set_commit_message(
    'duckpack',
    'duckpack apply: [v0.3.5] +2 create, ~1 alter'
);
```

Inspect audit history anytime via SQL or the Explorer sidebar:

```sql
SELECT snapshot_id, snapshot_time, author, commit_message, changes
FROM ducklake_snapshots('sales_lake')
ORDER BY snapshot_id DESC
LIMIT 20;
```

---

## Snapshot Explorer & Interactive Time-Travel Modal (`t`)

Every attached DuckLake catalog (`🌊`) in the Terminal IDE Explorer automatically displays a **`📸 Snapshots (N)`** node listing the 50 most recent commits (newest-first) with version numbers, timestamps, and commit messages.

- Press **`q`** or **`Enter`** on a snapshot row to inspect its metadata via `ducklake_snapshots('<catalog>')`.
- Press **`t`** anywhere on a DuckLake catalog, table, or snapshot to open the **Interactive Time-Travel & Snapshot Diff Modal**:

| Mode | Generated SQL |
| :--- | :--- |
| **By Version** | `SELECT * FROM "sales_lake"."orders" AT (VERSION => 14) LIMIT 100;` |
| **By Timestamp** | `SELECT * FROM "sales_lake"."orders" AT (TIMESTAMP => TIMESTAMPTZ '2026-11-12 18:00:00+00') LIMIT 100;` |
| **Table Changes (CDF)** | `SELECT * FROM ducklake_table_changes('sales_lake', 'main', 'orders', 13, 14) LIMIT 100;` |

Use `▲`/`▼` to pick snapshots straight from the live list, `◄`/`►` to switch tables or modes, and `Enter` to insert and run the query in your active editor tab.
