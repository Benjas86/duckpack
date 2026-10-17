<p align="center">
  <img src="duckpack_logo.png" width="400" alt="DuckPack Logo" />
</p>

<p align="center">
  <strong>Declarative Schema Management, Interactive Terminal IDE, and In-Memory Inspection for DuckDB</strong>
</p>

<p align="center">
  <a href="https://github.com/Benjas86/duckpack/releases"><img src="https://img.shields.io/github/v/release/Benjas86/duckpack?color=brightgreen&label=release" alt="GitHub Release"></a>
  <a href="https://pypi.org/project/duckpack/"><img src="https://img.shields.io/pypi/v/duckpack?color=blue&label=pypi" alt="PyPI Version"></a>
  <a href="https://pypi.org/project/duckpack/"><img src="https://img.shields.io/pypi/pyversions/duckpack" alt="Python Versions"></a>
  <a href="https://crates.io/crates/duckpack"><img src="https://img.shields.io/crates/v/duckpack?color=orange&label=crates.io" alt="Crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

---

DuckPack transforms how you develop with and manage DuckDB. Instead of maintaining fragile imperative `UP` and `DOWN` migration files, you define your desired database state as plain SQL files. DuckPack automatically computes the structural delta, detects renames heuristically, and applies non-destructive updates.

DuckPack includes a **rich terminal IDE** for live database exploration and first-class **Python bindings** to query and migrate in-memory databases directly inside your Python pipelines and test suites.

## 📌 Table of Contents

- [✨ Features](#-features)
- [🚀 Quick Start & Installation](#-quick-start--installation)
  - [Standalone Binary (Linux / macOS / Windows)](#1-standalone-binary)
  - [Python Package (pip / uv)](#2-python-package-pip--uv)
  - [Cargo (Rust Developers)](#3-cargo-rust-developers)
- [📋 CLI Command Reference](#-cli-command-reference)
- [🧑‍💻 DuckPack Terminal IDE](#-duckpack-terminal-ide)
  - [Keyboard Shortcuts Cheat Sheet](#keyboard-shortcuts-cheat-sheet)
  - [Catalog-Aware Multi-Database Exploration & Cross-DB Joins](#catalog-aware-multi-database-exploration--cross-db-joins)
  - [Live Database Attachment & Detach (`Ctrl+A` / `d`)](#live-database-attachment--detach-ctrla--d)
  - [Multi-Tab Query System & Isolated Results](#multi-tab-query-system--isolated-results)
  - [Interactive Schema Visualizer & Mermaid.js ERD](#interactive-schema-visualizer--mermaidjs-erd)
  - [Dynamic Status Bar & Error Interception](#dynamic-status-bar--error-interception)
- [🐍 Python Integration](#-python-integration)
  - [1. Interactive In-Memory Debugging (`explore`)](#1-interactive-in-memory-debugging)
  - [Jupyter Notebook & VS Code Pop-Out Terminal Support](#jupyter-notebook--vs-code-pop-out-terminal-support)
  - [2. Automated Testing with Pytest (`apply`)](#2-automated-testing-with-pytest)
  - [3. Pipeline Schema Drift Detection (`diff`)](#3-pipeline-schema-drift-detection)
  - [4. Python API Reference](#4-python-api-reference)
  - [CLI Python Management](#cli-python-management)
- [🦆 Quack Remote Server Integration](#-quack-remote-server-integration)
  - [Exploring Remote Quack Servers from CLI](#exploring-remote-quack-servers-from-cli)
  - [The `-- @remote` Query Directive](#the---remote-query-directive)
  - [SSL & Non-TLS Options](#ssl--non-tls-options)
- [📁 Declarative Project Structure](#-declarative-project-structure)
  - [Directory Layout](#directory-layout)
  - [Configuration (`duckpack.toml`)](#configuration-duckpacktoml)
  - [Environment Variables (`.env`)](#environment-variables-env)
  - [Ignoring Tables (`.duckdbignore`)](#ignoring-tables-duckdbignore)
- [📦 DuckPacks (`.duckpack`) for CI/CD](#-duckpacks-duckpack-for-cicd)
- [NOTE](#note)

---

## ✨ Features

- **Declarative Schemas:** Define your schemas, tables, views, and macros as raw `.sql` files. No migration timestamps or versions to manage.
- **Git-Style Smart Renames:** Uses Jaro-Winkler similarity matching (>80%) to detect renamed tables or columns, issuing non-destructive `ALTER TABLE RENAME` statements instead of dropping and recreating data.
- **Built-in Interactive Terminal IDE:** Explore live databases, write queries with syntax highlighting and autocompletion (`Ctrl+Space`), and manage independent query tabs.
- **Catalog-Aware Multi-Database Exploration:** Inspect multiple attached catalogs (local DuckDB, in-memory `memory` scratchpad, attached SQLite/Postgres/MySQL/MSSQL, and remote Quack `target_db` instances) in a unified hierarchical tree with catalog-qualified query generation and cross-database joins.
- **Live Catalog Attach & Detach (`Ctrl+A` / `d`):** Dynamically attach third-party databases, DuckDB files, or remote Quack instances directly inside the IDE without restarting.
- **Python Integration with Jupyter / VS Code Support:** Import `duckpack` into Python pipelines, pytest fixtures, or Jupyter notebooks (`duckpack.explore(con)`). Automatically spawns a connected native terminal window in non-interactive notebook environments.
- **In-Memory & Remote Attach:** Connect to local files, `:memory:` instances, MotherDuck (`md:...`), or remote DuckDB instances over the [Quack](https://duckdb.org/) protocol.
- **Remote Query Directive (`-- @remote`):** Forward session-scoped statements (e.g. S3 secrets, settings, and catalog scans) directly to remote engines with automated diagnostic hints.
- **ASCII & Mermaid.js Schema Visualizer:** Press `Ctrl+G` (or `F2`) to view table relationships and automatically copy formatted Mermaid.js Entity-Relationship diagrams to your system clipboard.
- **Environment Variable Templating:** Write `${ENV}_orders_${REGION}` directly in SQL; DuckPack dynamically interpolates values from `.env` files.
- **Immutable Snapshots (`.duckpack`):** Compile your entire project into a single, portable snapshot artifact for deterministic zero-dependency deployments.

---

## 🚀 Quick Start & Installation

### 1. Standalone Binary

Download pre-compiled binaries from the [GitHub Releases](https://github.com/Benjas86/duckpack/releases/latest) page, or install via terminal:

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

**macOS (Apple Silicon M1/M2/M3/M4):**
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
Download `duckpack-windows-amd64.exe` from [GitHub Releases](https://github.com/Benjas86/duckpack/releases/latest) and add it to your system `PATH`.

---

### 2. Python Package (`pip` / `uv`)

Installing DuckPack via Python installs **both** the Python package (`import duckpack`) and the standalone CLI binary (`duckpack` command in your virtualenv `bin/`):

```bash
# Using pip
pip install duckpack

# Using uv
uv add duckpack
```

*Requires Python ≥ 3.10. Pre-compiled wheels are available for Linux, macOS, and Windows.*

---

### 3. Cargo (Rust Developers)

```bash
cargo install duckpack
```

---

## 📋 CLI Command Reference

| Command | Description | Key Arguments / Flags | Default |
| :--- | :--- | :--- | :--- |
| `duckpack init` | Scaffold a new declarative database project | `-p, --project-dir <DIR>` | `.` |
| `duckpack build` | Validate SQL syntax against an in-memory Shadow DB | `-p, --project-dir <DIR>` | `.` |
| `duckpack apply` | Compute diff and apply migrations to target database | `-p, --project-dir <DIR>`<br>`-d, --db <URI>`<br>`--auto-approve`<br>`--force-drop`<br>`--out <FILE>`<br>`--quack-token <TOKEN>`<br>`--insecure` | `.`<br>`local.duckdb`<br>`false`<br>`false`<br>`None`<br>`None`<br>`false` |
| `duckpack explore` | Open the interactive terminal IDE / query editor | `-p, --project-dir <DIR>`<br>`-d, --db <URI>`<br>`--remote <SSH_TARGET>`<br>`-P, --port <PORT>`<br>`--auto-install`<br>`--quack-token <TOKEN>`<br>`--insecure` | `.`<br>`local.duckdb`<br>`None`<br>`22`<br>`false`<br>`None`<br>`false` |
| `duckpack compile` | Package project into an immutable `.duckpack` snapshot | `-p, --project-dir <DIR>`<br>`-o, --out <FILE>` | `.`<br>*(Required)* |
| `duckpack deploy` | Deploy compiled project to a remote server over SSH | `-p, --project-dir <DIR>`<br>`-r, --remote <HOST>`<br>`-d, --db <URI>`<br>`-P, --port <PORT>`<br>`--auto-install` | `.`<br>*(Required)*<br>`local.duckdb`<br>`22`<br>`false` |
| `duckpack python status` | Check if Python bindings are installed in active environment | — | — |
| `duckpack python install` | Install/upgrade `duckpack` Python package via pip | `--upgrade` | `false` |

---

## 🧑‍💻 DuckPack Terminal IDE

DuckPack includes a fully integrated terminal IDE built with Ratatui and Crossterm.

Launch it directly:
```bash
duckpack explore --db local.duckdb
```
Or explore a remote Quack server:
```bash
duckpack explore --db quack:0.0.0.0:9494 --quack-token <TOKEN>
```
Or transition instantly from `duckpack apply` by pressing **`e`**.

```
┌ Explorer [R: Refresh | Ctrl+A: Attach | q: Query] ──┬─ [Tab 1: cross_join.sql] ── [+ New] ── [❌ Close] ────┐
│ ▼ 📁 target_db [Active] (quack)                     │ SELECT                                                │
│   ▼ 📦 Tables (2)                                   │   ro.order_id,                                        │
│     ├─ customers (1,450 rows)                       │   ro.amount,                                          │
│     │  ├─ customer_id (BIGINT)                      │   lu.user_email                                       │
│     │  └─ email (VARCHAR)                           │ FROM target_db.orders ro                              │
│     └─ orders (8,920 rows)                          │ JOIN memory.local_users lu                            │
│   ▼ 👁️ Views (1)                                    │   ON ro.customer_id = lu.id                           │
│     └─ active_orders                                │ LIMIT 100;                                            │
│ ▼ 📁 memory (duckdb)                                │                                                       │
│   ▼ 📦 Tables (1)                                   │                                                       │
│     └─ local_users (50 rows)                        │                                                       │
├─────────────────────────────────────────────────────┴───────────────────────────────────────────────────────┤
│ Query Results [Page 1/1] (Showing 50 rows)                                                                  │
│ order_id │ amount  │ user_email                                                                             │
│ 1001     │ 149.99  │ alice@example.com                                                                      │
│ 1002     │ 89.50   │ bob@example.com                                                                        │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Keyboard Shortcuts Cheat Sheet

| Scope | Shortcut | Action |
| :--- | :--- | :--- |
| **Global / Navigation** | `Tab` | Cycle focus between **Explorer**, **Editor**, and **Results** panes |
| | `?` | Toggle the Help Modal |
| | `Ctrl+C` / `Esc` | Cancel operation / exit modal / quit IDE |
| **Editor** | `Ctrl+E` / `F5` | **Execute query** in active tab (or execute highlighted selection) |
| | `Shift + Arrows` | Highlight text to isolate and execute *only* the selection on `Ctrl+E` |
| | `Ctrl+Space` | Trigger **Intelligent Autocomplete** (catalog-aware tables, views, columns) |
| | `Ctrl+F` | **Auto-format** SQL statement using SQL parser |
| | `Ctrl+S` | **Save** active query to `queries/<tab_name>.sql` |
| | `Ctrl+Y` / `Ctrl+C` | **Copy** selected text (or active cell/line) to system clipboard |
| | `Ctrl+V` | **Paste** from OS system clipboard into editor |
| | `Ctrl+G` / `F2` | Open **Schema Visualizer** (ASCII tree + copies Mermaid.js ERD to clipboard) |
| **Tabs** | `Ctrl+T` | Open a new blank query tab |
| | `Ctrl+W` | Close the active query tab |
| | `Ctrl+N` / `Ctrl+P` | Switch to the **Next** / **Previous** tab |
| **Explorer** | `Up` / `Down` | Navigate through databases, table groups, tables, and columns |
| | `Enter` / `Space` | Expand or collapse database/group/table column tree / open SQL file |
| | `q` / `s` | **Query table/view** (generates catalog-qualified SQL into editor) |
| | `Ctrl+A` | **Attach Database** dialog (DuckDB, SQLite, Postgres, MySQL, MSSQL, Quack) |
| | `d` | **Detach Database** from current session |
| | `r` / `R` | **Refresh catalog** from live/remote database (preserves open state) |
| **Results Grid** | `Arrows` | Scroll vertically through rows and horizontally through columns |
| | `Shift + ◄ / ►` | Fast horizontal scroll across wide result tables |
| | `Home` / `End` | Jump directly to the **First** / **Last** column |
| | `PgUp` / `PgDn` | Fast scroll vertically |
| | `n` / `p` | Navigate to **Next** / **Previous** page (500-row pagination chunks) |
| | `Ctrl+O` | Export active query results to a CSV file in `queries/` |
| **Mouse Support** | Mouse Wheel | Seamless vertical & horizontal scrolling in any focused pane |
| | Click Tabs | Click on tab headers to switch or click `[+ New]` / `[❌ Close]` buttons |

### Catalog-Aware Multi-Database Exploration & Cross-DB Joins

DuckPack dynamically detects your active catalogs:
- **Single-Database Mode:** When working with an isolated database file, DuckPack renders a streamlined flat view (`📦 Tables` and `👁️ Views`).
- **Multi-Database Mode:** When multiple databases are attached—such as when connecting to a remote Quack server, attaching an external database via `Ctrl+A`, or attaching in Python—DuckPack organizes the sidebar into an explicit catalog hierarchy:
  - `📁 <database_name> [Active] (<type>)`
  - The `[Active]` badge highlights the database currently selected as `current_database()`.
  - Pressing `q` or double-clicking any table/view automatically produces the catalog-qualified SQL (e.g. `SELECT * FROM target_db.orders LIMIT 500;` or `SELECT * FROM memory.local_users LIMIT 500;`).
  - **Cross-Database Joins:** Because DuckDB supports federated queries, you can join remote Quack tables, local scratchpad tables in `memory`, and attached databases directly in the editor:
    ```sql
    SELECT *
    FROM target_db.remote_orders ro
    JOIN memory.local_users lu ON ro.customer_id = lu.id;
    ```

### Live Database Attachment & Detach (`Ctrl+A` / `d`)

Press **`Ctrl+A`** from anywhere in the IDE to open the interactive **Attach Database** modal dialog:
- Attach local DuckDB files (`.duckdb`, `.db`, `.ddb`).
- Attach SQLite databases (`TYPE SQLITE`).
- Connect to remote PostgreSQL, MySQL, or Microsoft SQL Server instances.
- Attach remote DuckDB instances over Quack (`TYPE QUACK`).
- Select attached databases in the Explorer and press **`d`** to safely detach them from the live session.

---

## 🐍 Python Integration

DuckPack provides native Python bindings compiled via **PyO3**. It allows data engineers and developers to programmatically inspect in-memory DuckDB connections, run declarative migrations in test fixtures, and detect schema drift without leaving Python.

### 1. Interactive In-Memory Debugging

Drop into the full DuckPack terminal IDE during pipeline execution or inside a `breakpoint()` / `pdb` session:

```python
import duckdb
import duckpack

# In-memory DuckDB database
con = duckdb.connect()

# Populate data in memory
con.execute("CREATE TABLE users AS SELECT 1 AS id, 'Alice' AS name;")
con.execute(
    "CREATE TABLE telemetry AS SELECT range AS id, random() AS metric FROM"
    " range(1000);"
)

# Launches the interactive DuckPack IDE directly in your current terminal:
duckpack.explore(con)

# When you quit the TUI (Esc / q), Python resumes execution seamlessly!
print("Resuming pipeline execution...")
```

### Jupyter Notebook & VS Code Pop-Out Terminal Support

Running terminal applications inside Jupyter notebooks (such as in VS Code Interactive, JupyterLab, or Jupyter Notebooks) usually fails with `os error 6: No such device or address` because notebooks lack an interactive TTY.

DuckPack solves this seamlessly:
- When you call `duckpack.explore(con)` inside a notebook cell, DuckPack automatically spins up an ephemeral local Quack bridge on a free local port, attaches to your live in-memory connection `con`, and **spawns a native graphical terminal emulator window** (Ghostty, Foot, Kitty, WezTerm, Alacritty, GNOME Terminal, Konsole, Windows Terminal `wt.exe`, macOS Terminal.app / iTerm2).
- You get the **full interactive TUI IDE experience** directly connected to your live in-memory notebook tables!
- When you exit the IDE window (`Esc` / `q`), the temporary bridge shuts down and notebook execution continues.
- In headless environments (CI / Docker without display), DuckPack falls back gracefully to printing a formatted schema overview directly into the notebook output cell.

```python
import duckdb
import duckpack

con = duckdb.connect()
con.execute(
    "CREATE TABLE metrics AS SELECT range AS id, random() AS val FROM"
    " range(500);"
)

# Automatically pops open an external connected terminal window:
duckpack.explore(con)

# Optional controls:
# duckpack.explore(con, open_window=False)  # Force in-cell formatted overview (headless mode)
# duckpack.explore(con, wait=False)         # Run terminal asynchronously in background
```

---

### 2. Automated Testing with Pytest

Maintain a single declarative SQL project and automatically apply it to fresh in-memory test databases:

```python
import duckdb
import duckpack
import pytest


@pytest.fixture
def db():
  # Spin up an isolated in-memory test database
  con = duckdb.connect()

  # Apply all schemas, tables, views, and macros from your declarative project:
  result = duckpack.apply(con, project_dir="./duckdb_project", auto_approve=True)
  assert result.success

  yield con
  con.close()


def test_user_creation(db):
  db.execute("INSERT INTO users (id, name) VALUES (42, 'Bob');")
  count = db.execute("SELECT count(*) FROM users;").fetchone()[0]
  assert count == 1
```

---

### 3. Pipeline Schema Drift Detection

Check if an analytical database matches your version-controlled SQL definitions before running ETL/ELT pipelines:

```python
import duckdb
import duckpack

con = duckdb.connect("production.duckdb")

# Detect schema drift
diff = duckpack.diff(con, project_dir="./duckdb_project")

if diff.has_changes:
  print(f"Schema drift detected ({diff.change_count} changes):")
  for change in diff.items:
    print(f" - {change}")
  raise RuntimeError("Database schema does not match codebase!")
```

---

### 4. Python API Reference

```python
import duckpack
```

#### Functions

- **`duckpack.explore(target, project_dir=".", quack_token=None, insecure=False, open_window=True, wait=True) -> None`**
  Launches the interactive TUI IDE against a live connection (`duckdb.DuckDBPyConnection`) or database URI (`str`). In non-TTY environments (Jupyter/VS Code), automatically launches a connected terminal window.

- **`duckpack.apply(target, project_dir=".", auto_approve=True, force_drop=False, interactive=False) -> ApplyResult`**
  Computes schema diffs and applies declarative migrations. Returns an `ApplyResult`.

- **`duckpack.diff(target, project_dir=".") -> DiffSummary`**
  Compares declarative project state against target database. Returns a `DiffSummary`.

- **`duckpack.compile(project_dir, out) -> None`**
  Compiles project SQL definitions into an immutable `.duckpack` snapshot file.

- **`duckpack.build(project_dir=".") -> None`**
  Validates SQL syntax and dependencies against an in-memory Shadow DB.

- **`duckpack.init(project_dir=".") -> None`**
  Scaffolds a new declarative database project directory.

#### Objects & Exceptions

- **`ApplyResult`**: Has attributes `.success: bool`, `.applied_count: int`, and `.items: list[str]`.
- **`DiffSummary`**: Has attributes `.has_changes: bool`, `.change_count: int`, and `.items: list[str]`.
- **Exceptions**: `duckpack.Error` (base), `duckpack.SchemaError`, `duckpack.ExecutionError`.

---

### CLI Python Management

Check and install the Python bindings directly from the standalone DuckPack CLI:

```bash
# Verify Python extension status in active environment
duckpack python status

# Install or upgrade duckpack in active environment via pip
duckpack python install --upgrade
```

---

## 🦆 Quack Remote Server Integration

DuckPack includes first-class integration with remote DuckDB instances running the [Quack](https://duckdb.org/) extension over HTTP/HTTPS.

```bash
# Explore a remote Quack server with authentication token
duckpack explore --db quack:localhost:9494 --quack-token <token>
```
*(You can also set `DUCKDB_QUACK_TOKEN=<token>` in your environment).*

### Exploring Remote Quack Servers from CLI

When you connect DuckPack to a remote Quack server from the CLI:

```bash
duckpack explore --db quack:0.0.0.0:9494 --quack-token <token>
```

DuckPack establishes a dual-catalog session:
1. **`target_db [Active] (quack)`**: The primary remote Quack database hosting remote tables, views, and persistent data.
2. **`memory (duckdb)`**: A fast, local client-side in-memory scratchpad database.

#### Dual Root Hierarchical Explorer
The Explorer pane automatically switches to multi-database mode, grouping assets under their respective root database folders:
- Navigate down to any table in `target_db` or `memory` and press **`q`** or **`s`** (or press Enter) to generate catalog-qualified queries directly into your active query editor tab:
  ```sql
  -- Generated by pressing 'q' on a target_db table:
  SELECT * FROM target_db.remote_table LIMIT 500;

  -- Generated by pressing 'q' on a memory table:
  SELECT * FROM memory.local_scratchpad LIMIT 500;
  ```

#### Cross-Database Joins Across Remote and Local
Because DuckDB evaluates queries client-side across all attached catalogs, you can write federated joins combining remote Quack datasets with local client tables in a single query:
```sql
SELECT 
    rt.order_id,
    rt.amount,
    mem.customer_category
FROM target_db.orders rt
JOIN memory.customer_categories mem ON rt.customer_id = mem.customer_id;
```
Standard `SELECT` queries, aggregations, cross-database joins, and DML run natively across the connection without requiring any special annotations.

### The `-- @remote` Query Directive

In DuckDB's engine architecture, session-scoped entities like **Secrets** and **Settings** are kept in local client memory by default. Prepending `-- @remote` at the top of your query instructs DuckPack to automatically forward the statement directly to the remote server engine via `FROM query(...)`:

```sql
-- @remote
CREATE SECRET (
    TYPE S3,
    KEY_ID 'AKIAIOSFODNN7EXAMPLE',
    SECRET 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY',
    REGION 'us-east-1'
);
```

| Statement Type | Needs `-- @remote`? | Execution Target | Behavior |
| :--- | :---: | :--- | :--- |
| `CREATE TABLE` / `CREATE VIEW` | **No** | Remote Database | Standard SQL; persists directly on remote Quack DB. |
| `SELECT * FROM table / view` | **No** | Remote Database | Standard SQL; queries remote data directly. |
| `INSERT` / `UPDATE` / `DELETE` | **No** | Remote Database | Standard SQL; mutates remote data directly. |
| **`CREATE SECRET (TYPE S3 ...)`** | **YES** | Remote Server Instance | Pushes S3/GCS/Azure credentials directly to the remote daemon. |
| **`SHOW TABLES` / `DESCRIBE`** | **YES** | Remote Server Instance | Executes server-side cleanly to introspect remote catalog. |
| **`SET variable = ...`** | **YES** | Remote Server Instance | Applies configuration variables directly to remote server session. |
| **`INSTALL / LOAD extension`** | **YES** | Remote Server Instance | Installs or loads extensions directly on remote Quack engine. |

### SSL & Non-TLS Options

DuckDB's Quack extension enforces HTTPS/SSL for remote hosts and uses plain HTTP for `localhost` and `127.0.0.1`.

If you are running a local development container (e.g. bound to `0.0.0.0:9494` or a Docker network IP) without TLS:
- `duckpack` **automatically detects `0.0.0.0` bindings** and disables SSL out-of-the-box.
- For other non-TLS endpoints, explicitly disable SSL using `--insecure` (or `--quack-disable-ssl`), or set `DUCKDB_QUACK_DISABLE_SSL=true`:
  ```bash
  duckpack explore --db quack:10.0.0.5:9494 --quack-token <token> --insecure
  ```

---

## 📁 Declarative Project Structure

### Directory Layout

Running `duckpack init --project-dir my_project` generates the standard declarative project scaffold:

```
my_project/
├── schemas/               # CREATE SCHEMA statements
├── tables/                # CREATE TABLE statements (one table per .sql file)
├── views/                 # CREATE VIEW statements (engine auto-forces OR REPLACE)
├── macros/                # CREATE MACRO statements
├── queries/               # Scratchpad queries, exports, and query history
├── seeds/                 # Static CSVs or data seed files
├── scripts/
│   ├── pre-deploy/        # Raw SQL executed BEFORE migrations are applied
│   └── post-deploy/       # Raw SQL executed AFTER migrations are applied
├── duckpack.toml          # Multi-environment targets configuration
├── .env                   # Environment variables for ${VAR} template substitution
└── .duckdbignore          # Ignore patterns for tables/views
```

### Configuration (`duckpack.toml`)

Define multiple environments to avoid hardcoding database paths:

```toml
[project]
name = "analytics_warehouse"
version = "0.2.0"

[target]
default_db = "dev.duckdb"

[env.dev]
db = "dev.duckdb"

[env.staging]
db = "staging.duckdb"

[env.prod]
db = "md:prod_db" # Points directly to MotherDuck!
```

Deploy or explore against any profile:
```bash
duckpack apply --env staging
duckpack explore --env prod
```

### Environment Variables (`.env`)

Write dynamic templates directly in your SQL files:
```sql
CREATE TABLE ${ENV}_customers_${REGION} (
    id INT PRIMARY KEY,
    name VARCHAR
);
```
DuckPack loads `.env` (or `.env.<profile>` when using `--env <profile>`) and interpolates variables via regex before compiling.

### Ignoring Tables (`.duckdbignore`)

Specify glob patterns for tables or views that DuckPack should ignore during schema diffs:
```
# Ignore ephemeral tables and temp backups
temp_*
*_backup
scratch.*
```

---

## 📦 DuckPacks (`.duckpack`) for CI/CD

For production pipelines and remote servers, applying migrations over a live network connection can be slow or risky. Compile your project into an immutable, offline snapshot artifact:

```bash
duckpack compile --project-dir my_project --out release_v1.duckpack
```

### How DuckPacks Work (Non-Destructively)

A `.duckpack` file is a pre-compiled DuckDB database containing your desired state with all environment variables resolved.

When you run `apply` using a DuckPack:
1. Opens `release_v1.duckpack` (Desired State).
2. Opens your live `prod.duckdb` (Current State).
3. **Calculates the Diff:** Compares the two databases in memory.
4. **Applies Non-Destructively:** Issues `ALTER TABLE`, `CREATE TABLE`, and `CREATE OR REPLACE VIEW` statements against the live database without dropping existing data.

Deploy to a remote server over SSH in a single step:
```bash
duckpack deploy --project-dir my_project --remote user@prod-server.internal --db /var/lib/duckdb/prod.duckdb --auto-install
```

---

## NOTE

I've spent time as an educational journey into Rust vibe coding this application. I have personal reasons to want to use this application and had never tried to 'vibe code' anything before.

I know this may contain 'sub-optimal' or 'vibe code' solutions and code that could be written more elegantly. I've also not fully tested everything yet so expect bugs, I'm fine with this! If you find this useful, want to tinker etc, please do.

Thanks,  
**Ben**
