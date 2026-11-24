[← Docs Index](./index.md)

# External Databases & Cloud Storage

DuckPack turns DuckDB v2.0's federation extensions into a seamless multi-database workspace — letting you attach, browse, refresh, and cross-join **Microsoft SQL Server (MSSQL)**, **PostgreSQL**, **MySQL**, **SQLite**, **MotherDuck**, and cloud object stores (**AWS S3**, **Cloudflare R2**, **GCS**, **Azure Blob**, **MinIO**) directly from `duckpack.toml` or the Terminal IDE.

---

## Engine Scope: SSDT vs. Query Federation

DuckPack maintains an intentional architectural boundary between native DuckDB/DuckLake engines and external relational engines:

| Capability | DuckDB / DuckLake / Quack / MotherDuck | External Engines (MSSQL, PostgreSQL, MySQL, SQLite) |
| :--- | :---: | :---: |
| **Terminal IDE Explorer & Column Inspection** | ✅ Full Support | ✅ Full Support |
| **Federated Cross-Database `JOIN`s & `SELECT`s** | ✅ Full Support | ✅ Full Support |
| **DML (`INSERT`, `UPDATE`, `DELETE`)** | ✅ Full Support | ✅ Full Support |
| **Live Metadata Cache Refresh (`r`)** | ✅ Full Support | ✅ Full Support |
| **Declarative Schema DDL (`diff`, `apply`)** | ✅ **Core Purpose** | ❌ **Intentionally Guarded** |

Attempting `duckpack apply --database mssql_db` halts safely with guidance to manage external engine DDL via native tools (SSDT, Flyway, Liquibase, Alembic) so engine-specific constraints, indexes, and collations are never stripped.

---

## Microsoft SQL Server (MSSQL)

DuckPack uses DuckDB's **native TDS `mssql` community extension** (no ODBC or JDBC drivers required) and automatically sets `SET mssql_exec_invalidate_cache = true;` on load.

### Supported Connection Forms

1. **Structured `duckpack.toml` (`[[attach]]` or `[env.<name>]`):**
   ```toml
   [[attach]]
   alias = "erp_mssql"
   type = "mssql"
   host = "sqlserver.corp.local"      # Also supports named instances: "myhost\\SQLEXPRESS"
   port = 1433                        # Omitted automatically for named instances when default 1433
   database = "erp_prod"
   username = "analytics_ro"
   password_env = "ERP_MSSQL_PASSWORD"
   insecure = true                    # Adds TrustServerCertificate=yes (for self-signed / Docker certs)
   read_only = true
   ```

2. **Windows Authentication (SSPI) / Linux & macOS Kerberos (`kinit`):**
   ```toml
   [[attach]]
   alias = "corp_dw"
   type = "mssql"
   host = "sql.corp.example.com"
   port = 1433
   database = "Sales"
   trusted_connection = true          # Emits Trusted_Connection=yes (never combined with Password=)
   insecure = true
   ```

3. **Native `mssql://` URI (via CLI `--db` or `dsn_env`):**
   ```bash
   duckpack explore --db "mssql://app:<password>@sql.example.com:1433/Sales?encrypt=true&trustservercertificate=true"
   ```

4. **Named Instances (`host\SQLEXPRESS`):**
   When `host` contains `\` and port is empty or `1433`, DuckPack emits `Server=myhost\SQLEXPRESS` (without `,1433`) so SQL Server Browser (UDP 1434) resolves the dynamic port automatically.

5. **Live Metadata Refresh (`r` in Explorer):**
   Runs `SELECT mssql_invalidate_cache('<alias>');` followed by `SELECT mssql_refresh_cache('<alias>');`.

---

## PostgreSQL

```toml
[[attach]]
alias = "crm_pg"
type = "postgres"
host = "pg.corp.internal"
port = 5432
database = "crm"
username = "analytics_ro"
password_env = "PG_CRM_PASSWORD"
disable_ssl = false                # Sets sslmode=disable when true
read_only = true
```

- Properly escapes special characters in libpq key-value connection strings.
- Pressing **`r`** in Explorer runs DuckDB v2.0's `CALL postgres_clear_cache();`.

---

## MySQL

```toml
[[attach]]
alias = "store_mysql"
type = "mysql"
host = "mysql.corp.internal"
port = 3306
database = "store"
username = "readonly"
password_env = "MYSQL_STORE_PASSWORD"
disable_ssl = false                # Sets ssl_mode=disabled when true
read_only = true
```

- Emits DuckDB `mysql` extension's required space-separated `host=... port=... database=... user=... password=...` format.
- Pressing **`r`** in Explorer runs `CALL mysql_clear_cache();`.

---

## SQLite

```toml
[[attach]]
alias = "legacy_sqlite"
type = "sqlite"
path = "./data/legacy.sqlite"
read_only = true
```

---

## MotherDuck

```toml
[[attach]]
alias = "md_warehouse"
type = "motherduck"
database = "analytics"
token_env = "MOTHERDUCK_TOKEN"
```

- Passes credentials via `?motherduck_token=...` on `ATTACH` (also available in `Ctrl+A` → MotherDuck).
- Pressing **`r`** in Explorer runs `REFRESH DATABASES;`.

---

## Cloud Secrets & S3 Bucket Explorer (`Ctrl+K`)

Press **`Ctrl+K`** inside `duckpack explore` to visually add or edit cloud credentials with masked inputs (`*`):
- **AWS S3** (`TYPE S3`)
- **Cloudflare R2** (automatically extracts `<account_id>` from `https://<account_id>.r2.cloudflarestorage.com` and creates a native `TYPE R2` secret with `ACCOUNT_ID`)
- **MinIO / Wasabi / Custom S3** (`TYPE S3` with `ENDPOINT`, `URL_STYLE 'path'`, `USE_SSL`)
- **Google Cloud Storage** (`TYPE GCS` via `[[secrets]]`)
- **Azure Blob / ADLS** (`TYPE AZURE` via `[[secrets]]`, auto-loading the `azure` extension)

### Dual Persistence & Workspace Round-Trip
- Secrets created in `Ctrl+K` can persist locally (`CREATE PERSISTENT SECRET` + `.env`) **and/or** save cleanly to `duckpack.toml` (`[[secrets]]` with `*_env` references) via **`[X] Save to duckpack.toml (Ctrl+S)`**.
- Explorer's **`🔐 Secrets & Cloud`** drawer lists all active secrets (`s3`, `r2`, `gcs`, `azure`, `quack`, `motherduck`, `http`, `ducklake`, `postgres`, `mysql`, `mssql`) and tracked buckets (`DUCKPACK_S3_BUCKET` / `DUCKPACK_S3_BUCKETS` / `[[secrets]].bucket`).
- Expand any bucket (`Enter`) to browse objects and virtual subdirectories via DuckDB `glob()`, and press **`q`** on a `.parquet`, `.csv`, or `.json` object to generate a ready-to-run query:

```sql
SELECT * FROM read_parquet('s3://analytics-lake/events/2026/part-000.parquet') LIMIT 100;
```

### Example: Cross-Database Federated Query

```sql
SELECT
    o.order_id,
    o.total_amount,
    c.company_name,
    e.click_count
FROM erp_mssql.dbo.orders o
JOIN crm_pg.public.customers c ON o.customer_id = c.id
LEFT JOIN read_parquet('s3://analytics-lake/clicks/*.parquet') e ON o.order_id = e.order_id
LIMIT 100;
```
