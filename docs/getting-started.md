[← Docs Index](./index.md)

# Getting Started

This guide walks you through installing DuckPack and running your first end-to-end declarative schema workflow in under 5 minutes.

---

## Installation

### 1. Standalone Binary (Linux / macOS / Windows)

Download pre-compiled binaries from [GitHub Releases](https://github.com/Benjas86/duckpack/releases/latest), or install via terminal:

**Linux (x86_64 / AMD64):**
```bash
curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-linux-amd64 -o duckpack
chmod +x duckpack
sudo mv duckpack /usr/local/bin/
```

**Linux (ARM64 / aarch64):**
```bash
curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-linux-arm64 -o duckpack
chmod +x duckpack
sudo mv duckpack /usr/local/bin/
```

**Linux (Musl Static — Alpine, Docker, minimal distros):**
```bash
curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-linux-amd64-static -o duckpack
chmod +x duckpack
sudo mv duckpack /usr/local/bin/
```

**macOS (Apple Silicon M1–M4):**
```bash
curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-macos-arm64 -o duckpack
chmod +x duckpack
sudo mv duckpack /usr/local/bin/
```

**macOS (Intel):**
```bash
curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-macos-amd64 -o duckpack
chmod +x duckpack
sudo mv duckpack /usr/local/bin/
```

**Windows:**
Download `duckpack-windows-amd64.exe` from [GitHub Releases](https://github.com/Benjas86/duckpack/releases/latest) and add it to your `PATH`.

---

### 2. Python Package (`pip` / `uv`)

Installing via Python installs **both** the Python library (`import duckpack`) and the `duckpack` CLI binary inside your virtual environment:

```bash
# Using uv
uv add duckpack

# Using pip
pip install duckpack
```

*Requires Python >= 3.10. Pre-compiled wheels are published for Linux, macOS, and Windows.*

If you already installed the standalone Rust CLI binary and want to add or check the Python bindings in your active virtual environment:

```bash
# Inspect detected Python interpreter, installer (uv/pip), and installed versions
duckpack python status

# Install or upgrade duckpack in the active environment
duckpack python install --upgrade
```

---

### 3. Cargo (Rust Developers)

```bash
cargo install duckpack
```

Verify your installation and inspect the embedded DuckDB engine version:

```bash
duckpack --version
# duckpack v0.3.5 (DuckDB engine: v2.0.0)
```

---

## 5-Minute Walkthrough: Your First Declarative Project

### Step 1: Scaffold a Project

```bash
duckpack init --project-dir my_warehouse
cd my_warehouse
```

This creates the standard declarative layout (`schemas/`, `sequences/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`, `scripts/`, and `duckpack.toml`).

### Step 2: Declare Your Desired State in Plain SQL

Create a schema, a sequence, two related tables, and an analytical view:

```sql
-- schemas/core.sql
CREATE SCHEMA IF NOT EXISTS core;
```

```sql
-- sequences/order_id_seq.sql
CREATE SEQUENCE IF NOT EXISTS core.order_id_seq START 1000 INCREMENT BY 1;
```

```sql
-- tables/core.customers.sql
CREATE TABLE core.customers (
    customer_id INTEGER PRIMARY KEY,
    email VARCHAR UNIQUE NOT NULL,
    tier VARCHAR DEFAULT 'standard'
);
```

```sql
-- tables/core.orders.sql
CREATE TABLE core.orders (
    order_id BIGINT DEFAULT nextval('core.order_id_seq') PRIMARY KEY,
    customer_id INTEGER REFERENCES core.customers(customer_id),
    amount DECIMAL(12, 2) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT current_timestamp
);
```

```sql
-- views/core.customer_spend.sql
CREATE VIEW core.customer_spend AS
SELECT
    c.customer_id,
    c.email,
    count(o.order_id) AS order_count,
    coalesce(sum(o.amount), 0) AS total_spend
FROM core.customers c
LEFT JOIN core.orders o ON o.customer_id = c.customer_id
GROUP BY c.customer_id, c.email;
```

### Step 3: Validate Offline Against an In-Memory Shadow DB

```bash
duckpack build
```

DuckPack compiles all SQL files in topological order (`schemas` → `types` → `sequences` → `macros` → `tables` → `views` → `triggers`) inside an isolated in-memory DuckDB instance to catch syntax and dependency errors before touching any real database.

### Step 4: Preview Pending Migrations (`diff`)

```bash
duckpack diff --db warehouse.duckdb
```

### Step 5: Apply Interactively (`apply`)

```bash
duckpack apply --db warehouse.duckdb
```

By default, `duckpack apply` opens an interactive diff review TUI showing every statement to be executed. Press `Space` to toggle items, `y` (or `Enter`) to apply, `n` / `Esc` to cancel, or `e` to jump straight into the Terminal IDE.

For non-interactive scripts or CI/CD, add `--auto-approve`:

```bash
duckpack apply --db warehouse.duckdb --auto-approve
```

### Step 6: Evolve Your Schema Safely

Now edit `tables/core.customers.sql` to rename `tier` to `customer_tier` and add a new column `country VARCHAR DEFAULT 'US'`:

```sql
-- tables/core.customers.sql
CREATE TABLE core.customers (
    customer_id INTEGER PRIMARY KEY,
    email VARCHAR UNIQUE NOT NULL,
    customer_tier VARCHAR DEFAULT 'standard',
    country VARCHAR DEFAULT 'US'
);
```

Run `duckpack diff --db warehouse.duckdb` — DuckPack automatically detects the rename via Jaro-Winkler similarity and generates non-destructive `ALTER TABLE` statements (`RENAME COLUMN` + `ADD COLUMN`) rather than dropping data.

### Step 7: Explore Live in the Terminal IDE

```bash
duckpack explore --db warehouse.duckdb
```

- Press **`F3`** to browse live catalogs, tables, PK/FK column icons (`🔑`, `🔗`), and run SQL (`Ctrl+E` / `F5`).
- Press **`F4`** to switch to **Project Authoring Mode**, add new assets (`a`), validate (`v`), and run in-IDE Diff & Apply (`F6`).

---

## Quick Start in Python

```python
import duckdb
import duckpack

con = duckdb.connect()

# Apply your declarative project to a fresh in-memory DuckDB connection
res = duckpack.apply(con, project_dir="./my_warehouse", auto_approve=True)
print(f"Applied {res.applied_count} changes")

# Insert sample data
con.execute("INSERT INTO core.customers (customer_id, email) VALUES (1, 'alice@example.com');")
con.execute("INSERT INTO core.orders (customer_id, amount) VALUES (1, 149.99);")

# Drop into the full interactive Terminal IDE connected to this live in-memory session
duckpack.explore(con, project_dir="./my_warehouse")
```

---

## Next Steps

- Learn every declarative SQL feature in **[Declarative Projects & Authoring](./declarative-projects.md)**
- Master the TUI shortcuts in **[Terminal IDE Guide](./terminal-ide.md)**
- Configure multi-environment profiles & multi-catalog workspaces in **[Workspace & Configuration](./workspace-and-configuration.md)**
