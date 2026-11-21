[← Docs Index](./index.md)

# Declarative Projects & Authoring

Instead of maintaining numbered imperative migration scripts (`001_up.sql`, `001_down.sql`), DuckPack follows the **SQL Server Data Tools (SSDT)** paradigm: you maintain the desired state of your database as plain `.sql` files in Git, and DuckPack computes and applies the minimal delta automatically.

---

## Canonical Directory Layout

Running `duckpack init --project-dir my_project` scaffolds the standard project structure:

```text
my_project/
├── schemas/               # CREATE SCHEMA statements (including nested schemas)
├── types/                 # Custom ENUM / TYPE definitions
├── sequences/             # CREATE SEQUENCE statements (compiled before tables)
├── macros/                # Scalar & table macros (CREATE MACRO ... AS [TABLE] ...)
├── tables/                # CREATE TABLE statements (desired schema state)
├── views/                 # CREATE [SECURE] VIEW statements
├── triggers/              # CREATE TRIGGER statements (DuckDB v2.0+)
├── seeds/                 # Reference CSV/SQL seed files
├── queries/               # Saved exploratory queries & IDE scratchpads
├── scripts/
│   ├── pre-deploy/        # Imperative SQL executed BEFORE schema migrations
│   └── post-deploy/       # Imperative SQL executed AFTER schema migrations
├── duckpack.toml          # Multi-environment targets, [[secrets]], [[attach]], [quack_auth]
├── .env                   # Local credentials & ${VAR} interpolation values (gitignored)
└── .duckpackignore        # Glob rules for ignored objects & one-time deploy scripts
```

### Topological Compilation Order

During `duckpack build`, `duckpack diff`, `duckpack apply`, and `duckpack compile`, DuckPack compiles project directories in strict dependency order with automatic intra-directory retry passes for cross-object dependencies:

1. `schemas/` (sorted shallowest-to-deepest so parent schemas exist before nested child schemas)
2. `types/`
3. `sequences/`
4. `macros/`
5. `tables/`
6. `views/`
7. `triggers/`

When dropping removed objects (`--force-drop`), DuckPack reverses dependencies cleanly: obsolete triggers are dropped before table/view/macro drops, tables before sequences, and child schemas before parent schemas.

---

## DuckDB v2.0 Authoring Features

### 1. First-Class Sequences & Surrogate Keys

DuckDB validates sequence references eagerly when creating tables with `DEFAULT nextval(...)`. Define sequences in `sequences/`:

```sql
-- sequences/invoice_id_seq.sql
CREATE SEQUENCE IF NOT EXISTS billing.invoice_id_seq
    START 10000
    INCREMENT BY 1;
```

```sql
-- tables/billing.invoices.sql
CREATE TABLE billing.invoices (
    invoice_id BIGINT DEFAULT nextval('billing.invoice_id_seq') PRIMARY KEY,
    customer_id INTEGER NOT NULL,
    total DECIMAL(12, 2) NOT NULL
);
```

- **Zero False Drift on Runtime Advancement:** DuckPack normalizes away DuckDB's live `START <next_counter>` value during diffing, so calling `nextval(...)` in production never triggers bogus sequence drift.
- **Option Changes:** Modifying `INCREMENT BY`, `MINVALUE`, `MAXVALUE`, or `CYCLE` emits `CREATE OR REPLACE SEQUENCE`.

### 2. Arbitrary Nested Schemas

DuckDB v2.0 supports multi-level nested schemas. Organize files either in nested directories (`tables/finance/reports/q3_kpis.sql`) or dot-named files (`tables/finance.reports.q3_kpis.sql`):

```sql
-- schemas/finance.sql
CREATE SCHEMA IF NOT EXISTS finance;
CREATE SCHEMA IF NOT EXISTS finance.reports;
```

```sql
-- tables/finance/reports/q3_kpis.sql
CREATE TABLE finance.reports.q3_kpis (
    metric_id INTEGER PRIMARY KEY,
    metric_name VARCHAR NOT NULL,
    value DOUBLE
);
```

DuckPack resolves nested schemas recursively via `duckdb_schemas().parent_schema_oid` across both local DuckDB and remote Quack catalogs.

### 3. Scalar Macros & Table Macros

Both scalar macros and DuckDB v2.0 table macros (`AS TABLE`) are introspected, diffed, and updated via `CREATE OR REPLACE MACRO`:

```sql
-- macros/double_all.sql
-- Use DuckDB 2.0 `lambda x: ...` syntax (single-arrow `x -> ...` is deprecated in v2.0)
CREATE MACRO core.double_all(arr) AS list_transform(arr, lambda x: x * 2);
```

```sql
-- macros/recent_orders.sql
CREATE MACRO core.recent_orders(days_back) AS TABLE
SELECT *
FROM core.orders
WHERE created_at >= current_timestamp - INTERVAL (days_back) DAY;
```

### 4. `CREATE SECURE VIEW` & Native `VARIANT` Columns

- **Secure Views:** Declaring `CREATE SECURE VIEW ...` in `views/` automatically emits `CREATE OR REPLACE SECURE VIEW ...` on updates so view security boundaries are preserved.
- **`VARIANT` Type:** Native `VARIANT` columns and `::JSON::VARIANT` casts are supported out of the box, with in-place `ALTER TABLE ... ALTER col TYPE VARIANT`.

### 5. DuckDB v2.0 Triggers & Automatic Preservation

Define triggers in `triggers/`:

```sql
-- triggers/orders_audit.sql
CREATE TRIGGER trg_orders_audit
AFTER INSERT ON core.orders
FOR EACH ROW
INSERT INTO core.order_audit_log VALUES (NEW.order_id, current_timestamp);
```

- **Per-Table Scoping:** DuckPack keys triggers by `(schema, table, trigger_name)` to match DuckDB v2.0's per-table trigger scoping — two tables in the same schema can safely share a trigger name like `trg_audit`.
- **In-Place Updates:** Modifying a trigger emits `CREATE OR REPLACE TRIGGER`.
- **Swap Preservation:** When a parent table undergoes a copy-and-swap recreation (`RecreateTable`), DuckPack automatically re-emits all triggers attached to that table immediately after the swap so triggers are never silently lost on cascade.

---

## Smart Diffing: Renames, Constraints & Copy-and-Swap

### Heuristic Rename Detection

DuckPack compares dropped and added columns/tables using **Jaro-Winkler similarity (> 0.80)**:
- **Columns:** Renaming `user_email` → `email_address` emits `ALTER TABLE ... RENAME COLUMN user_email TO email_address;`.
- **Tables (Same-Schema Only):** Renaming `sales.orders` → `sales.orders_v2` emits `ALTER TABLE "sales"."orders" RENAME TO "orders_v2";`. Cross-schema moves (`sales.orders` → `archive.orders`) are never misclassified as invalid cross-schema `ALTER TABLE RENAME`.

### Constraint Diffing & Copy-and-Swap Migrations

DuckPack introspects `duckdb_constraints()` for `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, and `CHECK`:

| Change | Generated SQL | Classification |
| :--- | :--- | :--- |
| Add `PRIMARY KEY` (none existed) | `ALTER TABLE t ADD PRIMARY KEY (...);` | Safe (`AddConstraint`) |
| Add `UNIQUE` | `ALTER TABLE t ADD UNIQUE (...);` | Safe (`AddConstraint`) |
| Add `FOREIGN KEY` or `CHECK` | Atomic transactional copy-and-swap (`__duckpack_swap`) | Protected (`RecreateTable`) |
| Drop or modify any existing constraint | Atomic transactional copy-and-swap (`__duckpack_swap`) | Protected (`RecreateTable`) |

Example generated copy-and-swap transaction:

```sql
BEGIN TRANSACTION;
CREATE TABLE orders__duckpack_swap (
    order_id INTEGER PRIMARY KEY,
    user_id INTEGER REFERENCES users(user_id),
    amount DECIMAL(18, 2) CHECK (amount >= 0)
);
INSERT INTO orders__duckpack_swap ("amount", "order_id", "user_id")
    SELECT "amount", "order_id", "user_id" FROM orders;
DROP TABLE "orders";
ALTER TABLE "orders__duckpack_swap" RENAME TO "orders";
COMMIT;
```

- **Generated Columns:** Columns marked `is_generated` (`GENERATED ALWAYS AS (...)`) are automatically excluded from the `INSERT INTO ... SELECT ...` copy list so swaps succeed cleanly.
- **Data Protection:** `RecreateTable` is deselected by default in interactive TUI review and requires `--force-drop` under `--auto-approve`. If existing data violates the new constraint, DuckDB rolls back the entire transaction automatically.
- **DuckLake Note:** DuckLake only supports `NOT NULL` constraints; DuckPack automatically ignores unsupported `PRIMARY KEY`/`UNIQUE`/`FOREIGN KEY`/`CHECK` constraints when diffing against a DuckLake target (see [DuckLake Guide](./ducklake.md)).

---

## Environment Variable Templating (`.env`)

Embed `${VARIABLE_NAME}` placeholders in any `.sql` file:

```sql
-- tables/events.sql
CREATE TABLE ${ENV}_events_${REGION} (
    event_id BIGINT PRIMARY KEY,
    payload VARIANT
);
```

Define values in `.env` (and optional environment-specific overrides in `.env.staging`, `.env.prod` loaded via `--env <profile>`):

```bash
# .env
ENV=dev
REGION=eu
```

DuckPack loads `.env` first, overlays `.env.<profile>` when `--env` is supplied, and interpolates `${VAR}` before Shadow DB compilation.

---

## Pre-Deploy / Post-Deploy Scripts & `.duckpackignore`

Place imperative data backfills, extension installs, or one-off bootstrap statements in:
- `scripts/pre-deploy/*.sql` — executed before schema migrations
- `scripts/post-deploy/*.sql` — executed after schema migrations

Use `.duckpackignore` (legacy `.duckdbignore` / `.duckignore` also supported) to ignore ephemeral tables, scratch schemas, or one-time deploy scripts after their initial run:

```gitignore
# Ignore ephemeral tables, backups, or schemas
temp_*
*_backup
scratch.*

# Ignore one-time deployment scripts after they have run
scripts/pre-deploy/01_initial_seed.sql
scripts/post-deploy/02_backfill_legacy.sql

# Ignore an entire directory or wildcard path
archive/
scripts/post-deploy/legacy_*
```

---

## Multi-Database Catalog Targeting

When a connection has multiple attached catalogs (e.g. `ATTACH 'analytics.duckdb' AS analytics;`), target a specific catalog using this precedence hierarchy:

1. **CLI flag:** `--database <NAME>` (alias `--catalog <NAME>`)
2. **Python kwarg:** `duckpack.apply(con, database="analytics")` / `duckpack.diff(con, database="analytics")`
3. **Config file:** `duckpack.toml` (`[target]` or `[env.<name>]` `database = "analytics"`)
4. **Fallback:** Active `current_database()`

DuckPack isolates introspection and DDL exclusively to the targeted catalog (`database_name = '<name>'`). Attempting to run `apply` against an external federated engine (`mssql`, `postgres`, `mysql`, `sqlite`) is intentionally blocked with a clear diagnostic message (see [External Databases](./external-databases.md#engine-scope-ssdt-vs-query-federation)).
