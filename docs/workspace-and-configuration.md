[← Docs Index](./index.md)

# Workspace & Configuration (`duckpack.toml`)

`duckpack.toml` is the single declarative configuration file for your project's target databases, multi-environment profiles (`--env`), cloud secrets (`[[secrets]]`), multi-catalog workspace attachments (`[[attach]]`), and Quack RBAC policies (`[quack_auth]`).

---

## Single-Target & Multi-Environment Configuration

Define `[target]` for your default environment and `[env.<name>]` profiles for `dev`, `staging`, `lakehouse`, `prod`, etc.:

```toml
[project]
name = "analytics_warehouse"
version = "0.3.5"

[target]
default_db = "dev.duckdb"
database = "analytics"        # Optional: default attached catalog name for diff/apply

[env.dev]
db = "dev.duckdb"

[env.staging]
db = "quack:staging.company.internal:9494"
database = "analytics_stg"
token_env = "STAGING_QUACK_TOKEN"
insecure = true               # Plain HTTP for internal / self-signed clusters

[env.lakehouse]
provider = "ducklake"
catalog_dsn = "quack:lake-catalog.internal:9494"
data_path = "s3://warehouse/gold/"
inlining_limit = 1000
token_env = "LAKE_QUACK_TOKEN"
insecure = true

[env.sqlserver]
provider = "mssql"
host = "sqlserver.corp.local"
port = 1433
database = "analytics_dw"
trusted_connection = true     # Windows SSPI / POSIX Kerberos (or use username + password_env)
trust_server_certificate = true

[env.prod]
db = "quack:analytics.company.com:443"
database = "analytics"
token_env = "PROD_QUACK_TOKEN"
```

Activate any environment with `--env <profile>` (CLI) or `env="<profile>"` (Python):

```bash
duckpack explore --env staging
duckpack diff --env prod
duckpack apply --env prod --auto-approve
```

---

## Declarative Workspace Secrets (`[[secrets]]`)

Declare cloud and HTTP secrets once in `duckpack.toml` so they are auto-loaded across `explore`, `diff`, `apply`, and the Python API (`duckpack.explore()`, `duckpack.apply()`, `duckpack.diff()`).

### Strict Credential Hygiene

`duckpack.toml` is designed to be committed to Git. **Raw credential keys (`secret`, `secret_key`, `password`, `pwd`, `token`, `admin_token`, `key_id`, `access_key_id`, `dsn`, `connection_string`) are strictly rejected.** Always reference environment variable names via `*_env` fields and store actual values in `.env` (which DuckPack writes with `0600` permissions on Unix).

### Supported `[[secrets]]` Types

#### AWS S3 / MinIO / S3-Compatible (`type = "s3"`)
```toml
[[secrets]]
name = "lake_minio"
type = "s3"
scope = "s3://lakehouse/"         # Optional per-bucket/prefix SCOPE isolation
endpoint_env = "MINIO_ENDPOINT"   # e.g. localhost:9000 (or static `endpoint = "..."`)
key_id_env = "MINIO_ACCESS_KEY"
secret_env = "MINIO_SECRET_KEY"
region = "us-east-1"
url_style = "path"                # "path" for MinIO/local, "vhost" for AWS S3
use_ssl = false
bucket = "s3://lakehouse"         # Optional: auto-registers bucket in Explorer
```

#### Cloudflare R2 (`type = "r2"`)
```toml
[[secrets]]
name = "cloudflare_r2"
type = "r2"
scope = "r2://analytics/"
endpoint = "https://<account_id>.r2.cloudflarestorage.com"  # Auto-extracts ACCOUNT_ID -> native TYPE R2
key_id_env = "R2_ACCESS_KEY_ID"
secret_env = "R2_SECRET_ACCESS_KEY"
```

#### Google Cloud Storage (`type = "gcs"`)
```toml
[[secrets]]
name = "gcp_lake"
type = "gcs"
scope = "gcs://my-gcp-bucket/"
key_id_env = "GCS_HMAC_KEY_ID"
secret_env = "GCS_HMAC_SECRET"
```

#### Azure Blob / ADLS (`type = "azure"`)
```toml
[[secrets]]
name = "azure_datalake"
type = "azure"
scope = "az://my-container/"
secret_env = "AZURE_STORAGE_CONNECTION_STRING"   # Or token_env for access_token / omitted for credential_chain
```

#### HTTP Bearer Token (`type = "http"`)
```toml
[[secrets]]
name = "internal_api"
type = "http"
scope = "https://data.internal.company.com/"
token_env = "DATA_API_BEARER_TOKEN"
```

#### Scoped Quack Token (`type = "quack"`)
```toml
[[secrets]]
name = "quack_sales"
type = "quack"
scope = "quack:sales-catalog.internal:9494"
token_env = "QUACK_SALES_TOKEN"
```

---

## Declarative Multi-Catalog Attachments (`[[attach]]`)

For multi-container architectures — such as **one DuckDB + Quack container per DuckLake catalog** backed by S3/MinIO/R2, alongside PostgreSQL, MSSQL, MySQL, SQLite, or MotherDuck sources — declare `[[attach]]` entries in `duckpack.toml`:

```toml
# DuckLake #1 backed by Quack container #1
[[attach]]
alias = "sales_lake"
type = "ducklake"
backend = "quack"
metadata = "quack:sales-catalog.internal:9494"
data_path = "s3://lakehouse/sales/"
token_env = "QUACK_SALES_TOKEN"
admin_token_env = "QUACK_SALES_ADMIN_TOKEN"  # Optional: elevated token used by Ctrl+U
client_id = "alice_admin"
insecure = true
default = true                                # Initial active catalog tab in Workspace Mode

# DuckLake #2 backed by Quack container #2 (fully isolated token!)
[[attach]]
alias = "finance_lake"
type = "ducklake"
backend = "quack"
metadata = "quack:finance-catalog.internal:9494"
data_path = "s3://lakehouse/finance/"
token_env = "QUACK_FINANCE_TOKEN"
client_id = "alice_admin"
insecure = true

# Direct Quack analytics server
[[attach]]
alias = "ops_quack"
type = "quack"
host = "ops-db.internal"
port = 9494
token_env = "QUACK_OPS_TOKEN"
client_id = "alice_admin"
insecure = true

# Federated read-only PostgreSQL source
[[attach]]
alias = "crm_pg"
type = "postgres"
host = "pg.corp.internal"
port = 5432
database = "crm"
username = "analytics_ro"
password_env = "PG_CRM_PASSWORD"
read_only = true

# Federated SQL Server source (Windows Auth / Kerberos)
[[attach]]
alias = "erp_mssql"
type = "mssql"
host = "sql.corp.internal"
port = 1433
database = "erp_prod"
trusted_connection = true
insecure = true                               # Sets TrustServerCertificate=yes
read_only = true

# Remote read-only frozen DuckLake over HTTPS
[[attach]]
alias = "tpch_public"
type = "ducklake"
backend = "file"
path = "https://blobs.duckdb.org/datalake/tpch-sf1.ducklake"
read_only = true
```

### Environment Layering (`[[env.<profile>.secrets]]` & `[[env.<profile>.attach]]`)

Environment-specific arrays merge with top-level entries by `name` (for secrets) or `alias` (for attachments). Set `enabled = false` to disable an inherited base entry in a specific environment:

```toml
[[env.prod.attach]]
alias = "sales_lake"
type = "ducklake"
backend = "quack"
metadata = "quack:sales-prod.company.com:443"
data_path = "s3://prod-lakehouse/sales/"
token_env = "QUACK_PROD_SALES_TOKEN"
client_id = "alice_admin"
insecure = false
default = true

# Disable local dev scratchpad in prod
[[env.prod.attach]]
alias = "dev_scratch"
enabled = false
```

### In-Memory Workspace Mode & Resilient Startup

- **Zero Stray `local.duckdb` Files:** When `[[attach]]` is defined and neither `--db` nor `[target].db` is set, `duckpack explore` (and Python `duckpack.explore()`) automatically opens a clean `:memory:` session and mounts every declared catalog.
- **Resilient Startup:** Offline services (`⚠️ sales_lake (offline — Enter/r to retry)`) and missing `*_env` vars (`🔒 finance_lake (missing QUACK_FINANCE_TOKEN)`) never abort startup. Shared team `duckpack.toml` files work even when a developer only holds tokens for a subset of catalogs.
- **Opt-In TUI Round-Trip (`Ctrl+S`):** Inside `Ctrl+A` (Attach Database) and `Ctrl+K` (Cloud Secrets), toggle **`[X] Save to duckpack.toml (Ctrl+S)`** to append/update the entry in `duckpack.toml` (respecting `--env`) and store its secret values in `.env` (`0600` permissions) automatically.

---

## `.env` Loading & Credential Precedence

DuckPack loads environment files in order:
1. `.env` (base defaults)
2. `.env.<profile>` (overrides base when `--env <profile>` is active)
3. Process environment variables (`export ...`)

For Quack authentication tokens, DuckPack resolves:
1. Explicit CLI `--quack-token` / Python `quack_token=` (not recommended for shared terminals — warns about shell history)
2. `token_env` declared in `duckpack.toml` (`[env.<profile>]`, `[target]`, or `[[attach]]`)
3. `QUACK_TOKEN`
4. `DUCKDB_QUACK_TOKEN`
