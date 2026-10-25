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

DuckPack transforms how you develop with and manage DuckDB. Instead of maintaining fragile imperative `UP` and `DOWN` migration files, you define your desired database state as plain SQL files. Acting as a modern **SQL Server Data Tools (SSDT) for DuckDB**, DuckPack automatically computes structural deltas, detects renames heuristically, and applies non-destructive schema migrations.

DuckPack includes a **rich terminal IDE** for live database exploration and first-class **Python bindings** to query, migrate, and inspect in-memory and persistent databases directly inside your Python pipelines, test suites, and Jupyter notebooks.

## 📌 Table of Contents

- [✨ Features](#-features)
- [🚀 Quick Start & Installation](#-quick-start--installation)
  - [Standalone Binary (Linux / macOS / Windows)](#1-standalone-binary)
  - [Python Package (pip / uv)](#2-python-package-pip--uv)
  - [Cargo (Rust Developers)](#3-cargo-rust-developers)
- [📋 CLI Command Reference](#-cli-command-reference)
- [🏛️ Architecture & Design Philosophy](#️-architecture--design-philosophy)
  - [DuckPack as "SSDT for DuckDB"](#duckpack-as-ssdt-for-duckdb)
  - [Engine Scope: DuckDB DDL vs External Query Federation](#engine-scope-duckdb-ddl-vs-external-query-federation)
  - [Multi-Database Catalog Targeting](#multi-database-catalog-targeting)
  - [Table Constraint Diffing & Copy-and-Swap Migrations](#table-constraint-diffing--copy-and-swap-migrations)
  - [Interactive Apply & Notebook Parity](#interactive-apply--notebook-parity)
  - [Embedded DuckDB Version Transparency & Diagnostics](#embedded-duckdb-version-transparency--diagnostics)
- [🧑‍💻 DuckPack Terminal IDE](#-duckpack-terminal-ide)
  - [Keyboard Shortcuts Cheat Sheet](#keyboard-shortcuts-cheat-sheet)
  - [Primary Key & Foreign Key Visual Identifiers](#primary-key--foreign-key-visual-identifiers)
  - [Dynamic Explorer Width Resizing & Snappy Scrolling](#dynamic-explorer-width-resizing--snappy-scrolling)
  - [Catalog-Aware Multi-Database Exploration & Cross-DB Joins](#catalog-aware-multi-database-exploration--cross-db-joins)
  - [Live Database Attachment with Provider Presets (`Ctrl+A` / `d`)](#live-database-attachment-with-provider-presets-ctrla--d)
  - [Cloud Secrets Management & S3 Bucket Explorer (`Ctrl+K`)](#cloud-secrets-management--s3-bucket-explorer-ctrlk)
  - [Multi-Tab Query System & Isolated Results](#multi-tab-query-system--isolated-results)
  - [Interactive Schema Visualizer & Mermaid.js ERD](#interactive-schema-visualizer--mermaidjs-erd)
  - [Dynamic Status Bar & Error Interception](#dynamic-status-bar--error-interception)
- [🐍 Python Integration](#-python-integration)
  - [1. Interactive In-Memory Debugging (`explore`)](#1-interactive-in-memory-debugging)
    - [Jupyter Notebook & VS Code Pop-Out Terminal Support](#jupyter-notebook--vs-code-pop-out-terminal-support)
  - [2. Declarative Migrations in Pytest & Pipelines (`apply`)](#2-declarative-migrations-in-pytest--pipelines-apply)
  - [3. Pipeline Schema Drift Detection (`diff`)](#3-pipeline-schema-drift-detection-diff)
  - [4. Multi-Database Catalog Targeting in Python](#4-multi-database-catalog-targeting-in-python)
  - [5. Python API Reference](#5-python-api-reference)
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

- **Declarative Schema Management ("SSDT for DuckDB"):** Define your schemas, tables, views, and macros as raw `.sql` files. No migration timestamps, sequence numbers, or rollback scripts to maintain.
- **Table Constraint Diffing & Copy-and-Swap Migrations:** Automatically introspects `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, and `CHECK` constraints. Generates in-place `ALTER TABLE` statements for safe additions and transactional copy-and-swap (`__duckpack_swap`) staging tables for complex constraint modifications.
- **Catalog-Aware Multi-Database Targeting:** Seamlessly target specific attached databases (via `--database <name>`, `duckpack.toml [target] database = "..."`, or Python `duckpack.apply(con, database="analytics")`) with strict catalog isolation.
- **Clear Engine Boundary (SSDT vs Federation):** Full declarative DDL lifecycle management specifically engineered for DuckDB, Quack, and MotherDuck. External attached engines (MSSQL, PostgreSQL, MySQL, SQLite) are first-class citizens in Explore for ad-hoc querying, cross-DB joins, and DML (`INSERT INTO`, `UPDATE`), with schema DDL safely managed by external native tools.
- **Interactive Apply Parity (CLI & Notebook):** Review visual colored diffs and apply migrations interactively with live confirmation prompts in terminal, Python scripts, or Jupyter notebooks (with automated external terminal pop-out).
- **Schema Drift Detection (`duckpack diff`):** Instant dry-run inspection to detect divergence between your declarative codebase and live databases without mutating state.
- **Git-Style Smart Renames:** Uses Jaro-Winkler similarity matching (>80%) to detect renamed tables or columns, issuing non-destructive `ALTER TABLE RENAME` statements instead of dropping and recreating data.
- **Built-in Interactive Terminal IDE:** Explore live databases, write queries with syntax highlighting and autocompletion (`Ctrl+Space`), and manage independent query tabs.
- **Unified IDE Authoring & Project Mode (`F3: DB | F4: Project`):** Toggle between live database catalog inspection and declarative project authoring. Scaffold tables, views, macros, and schemas (`a`/`n`), edit with dirty tab indicators (`*`), validate project syntax and dependencies against an in-memory Shadow DB (`Ctrl+B`/`v`), inspect passive Git status badges (`[M]`, `[+]`, `[D]`), and run declarative diff & apply (`F6`/`Ctrl+D`) directly without quitting the IDE.
- **Visual Primary Key & Foreign Key Indicators:** Inspect table structures in the Explorer tree with visual icons (`🔑` for Primary Keys, `🔗` for Foreign Keys with reference targets `[FK -> table.col]`, and composite `🔑🔗` badges).
- **Dynamic Explorer Width Resizing & Snappy Scrolling:** Adjust sidebar width on the fly via `W` (cycling 25%/35%/50% presets) or `+`/`-` and `[`/`]` (5% increments from 15% to 60%). Navigate wide schemas quickly with 4-char arrow scrolling, `Shift+Arrow` 10-char jumps, and `Home` margin snap.
- **Catalog-Aware Multi-Database Exploration:** Inspect multiple attached catalogs (local DuckDB, in-memory `memory` scratchpad, attached SQLite/Postgres/MySQL/MSSQL, and remote Quack `target_db` instances) in a unified hierarchical tree with catalog-qualified query generation and cross-database joins.
- **Live Catalog Attach with Provider Presets (`Ctrl+A` / `d` / `b`):** Dynamically attach third-party databases, DuckDB files, MotherDuck, or remote Quack instances directly inside the IDE with provider presets and auto-installed extensions; cycle target catalogs on the fly.
- **Cloud Secrets Management & S3 Bucket Explorer (`Ctrl+K`):** Visually configure cloud storage credentials (AWS S3, Cloudflare R2, MinIO, Wasabi, Custom S3) with masked inputs (`*`) and dual-persistence (`CREATE PERSISTENT SECRET` + `.env`). Live-explore remote S3 buckets (`glob()`) and sample remote files (`read_parquet`, `read_csv`) with a single keypress.
- **Temporary Table Discovery & Python Two-Way Sync:** Temporary tables and views (`CREATE TEMP TABLE ...`) appear seamlessly in the active database's Explorer tree with a distinct `[temp]` badge. Instant DDL auto-refresh updates the sidebar on `CREATE`/`DROP`/`ALTER`, and interactive Python exploration (`duckpack.explore(con)`) performs safe two-way session synchronization across the Quack bridge.
- **Python Integration with Jupyter / VS Code Support:** Import `duckpack` into Python pipelines, pytest fixtures, or Jupyter notebooks (`duckpack.explore(con)`). Automatically spawns a connected native terminal window in non-interactive notebook environments.
- **Embedded DuckDB Version Transparency:** Engine version reporting via `duckpack --version` and Python `__duckdb_version__`, backed by proactive storage format compatibility diagnostics.
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
| `duckpack diff` | Compare declarative project state against target database without applying changes | `-p, --project-dir <DIR>`<br>`-d, --db <URI>`<br>`--database, --catalog <NAME>`<br>`--quack-token <TOKEN>`<br>`--insecure` | `.`<br>`local.duckdb`<br>`None`<br>`None`<br>`false` |
| `duckpack apply` | Compute diff and apply migrations to target database | `-p, --project-dir <DIR>`<br>`-d, --db <URI>`<br>`--database, --catalog <NAME>`<br>`--auto-approve`<br>`--interactive`<br>`--force-drop`<br>`--out <FILE>`<br>`--quack-token <TOKEN>`<br>`--insecure` | `.`<br>`local.duckdb`<br>`None`<br>`false`<br>`false`<br>`false`<br>`None`<br>`None`<br>`false` |
| `duckpack explore` | Open the interactive terminal IDE / query editor | `-p, --project-dir <DIR>`<br>`-d, --db <URI>`<br>`--database, --catalog <NAME>`<br>`--remote <SSH_TARGET>`<br>`-P, --port <PORT>`<br>`--auto-install`<br>`--quack-token <TOKEN>`<br>`--insecure` | `.`<br>`local.duckdb`<br>`None`<br>`None`<br>`22`<br>`false`<br>`None`<br>`false` |
| `duckpack compile` | Package project into an immutable `.duckpack` snapshot | `-p, --project-dir <DIR>`<br>`-o, --out <FILE>` | `.`<br>*(Required)* |
| `duckpack deploy` | Deploy compiled project to a remote server over SSH | `-p, --project-dir <DIR>`<br>`-r, --remote <HOST>`<br>`-d, --db <URI>`<br>`-P, --port <PORT>`<br>`--auto-install` | `.`<br>*(Required)*<br>`local.duckdb`<br>`22`<br>`false` |
| `duckpack python status` | Check if Python bindings are installed in active environment | — | — |
| `duckpack python install` | Install/upgrade `duckpack` Python package via pip | `--upgrade` | `false` |

---

## 🏛️ Architecture & Design Philosophy

DuckPack is built on core design principles established to provide reliable, enterprise-grade declarative schema management and database exploration for the modern DuckDB ecosystem.

### DuckPack as "SSDT for DuckDB"

For decades, Microsoft's **SQL Server Data Tools (SSDT)** defined the gold standard for enterprise database lifecycle management: developers maintain their desired database state as plain SQL DDL files in source control, and a compiler/engine determines the exact delta and migration plan to bring a live database into alignment.

Traditional data tooling often relies on **imperative migration files** (`001_create_users.sql`, `002_add_email.sql`, `002_rollback.sql`). In analytical and serverless data environments, this model leads to friction:
- Migrations drift out of sync when schemas are iterated rapidly.
- Rollbacks are difficult or untested.
- Branch merges in Git create migration ordering conflicts.

**DuckPack brings the SSDT paradigm to DuckDB:**
1. **Desired-State First:** You define the current desired state of tables, views, schemas, and macros as plain `.sql` files in your repository (`tables/users.sql`, `views/active_orders.sql`).
2. **Automated Delta Calculation:** DuckPack parses your codebase into an in-memory Shadow database, inspects the target database catalog, and computes the exact difference (tables to create, columns to alter, views to replace).
3. **Smart Heuristic Renames:** DuckPack employs Jaro-Winkler string similarity analysis (>80% threshold) on column signatures. If you rename `user_email` to `email_address`, DuckPack issues a non-destructive `ALTER TABLE RENAME COLUMN` rather than dropping the column and losing data.
4. **Deterministic & Safe Deployments:** In production and CI/CD pipelines, changes can be packaged into an immutable `.duckpack` snapshot artifact or verified dry-run via `duckpack diff`.

### Engine Scope: DuckDB DDL vs External Query Federation

DuckDB has powerful native federation capabilities: via official extensions, DuckDB can attach external relational engines such as PostgreSQL, MySQL, SQLite, and Microsoft SQL Server (MSSQL via ODBC).

A common question is: *Can DuckPack act as a declarative SSDT schema manager for MSSQL or Postgres through DuckDB?*

**The architectural boundary is strictly defined:**

| Capability | DuckDB Native / Quack / MotherDuck | External Engines (MSSQL, Postgres, MySQL, SQLite) |
| :--- | :---: | :---: |
| **Interactive Terminal IDE (Explore)** | ✅ Full Support | ✅ Full Support (browse catalogs, inspect tables/columns) |
| **Cross-Database Joins & Queries** | ✅ Full Support | ✅ Full Support (`SELECT * FROM mssql.tbl JOIN duckdb.tbl`) |
| **Data Manipulation (DML: `INSERT`, `UPDATE`)** | ✅ Full Support | ✅ Full Support |
| **Declarative Schema Management (`apply`, `diff`)** | ✅ **Core Purpose** | ❌ **Intentionally Excluded & Guarded** |

#### Why External DDL is Out of Scope for `apply`:
1. **Dialect Mismatches:** DuckDB's SQL parser and pushdown DDL cannot express enterprise engine-specific features such as SQL Server `IDENTITY(1,1)`, `CLUSTERED INDEX`, `NVARCHAR(MAX) FILESTREAM`, partitioned tables, or T-SQL triggers.
2. **Silent Failure & Loss of Semantics:** Applying DuckDB DDL to an attached external engine strips database-specific column constraints, default expressions, and index options.
3. **Guardrails:** If you accidentally attempt to run `duckpack apply --database mssql_db`, DuckPack inspects `duckdb_databases()`, detects the engine type, and halts immediately with an informative message guiding you to use native tools (SSDT, Flyway, Liquibase, Alembic) for external engines while keeping DuckPack for your DuckDB workloads.

### Multi-Database Catalog Targeting

Modern analytical setups attach multiple databases simultaneously (e.g. `ATTACH 'analytics.duckdb' AS analytics;`). DuckPack provides fine-grained catalog targeting across the CLI, configuration files, Python API, and TUI:

#### Resolution Hierarchy:
1. **CLI Flag:** `--database <NAME>` or `--catalog <NAME>` (e.g. `duckpack apply --database analytics`).
2. **Python Parameter:** `duckpack.apply(con, database="analytics")` or `duckpack.diff(con, database="analytics")`.
3. **Configuration File:** `duckpack.toml` target or environment setting:
   ```toml
   [target]
   database = "analytics"
   ```
4. **Active Fallback:** The active database returned by `current_database()` (or `memory` for standalone in-memory sessions).

#### Strict Catalog Isolation:
When a specific catalog is targeted:
- DuckPack introspects *only* tables, views, and macros belonging to that specific catalog (`database_name = '<name>'`), ignoring objects in other attached databases.
- Migrations are executed within the target database context using safe `USE "<database>"` switching, ensuring unqualified DDL statements deploy cleanly without touching default or scratchpad databases.
- In the TUI Explorer, press **`b`** to cycle between attached writable DuckDB catalogs for diff and apply operations.

### Table Constraint Diffing & Copy-and-Swap Migrations

DuckDB's internal `duckdb_columns()` view maps column names to data types, but does not capture relational constraints (`PRIMARY KEY`, `FOREIGN KEY`, `UNIQUE`, or `CHECK`). DuckPack bridges this by querying `duckdb_constraints()` across both your declarative models (in the in-memory shadow database) and the target database:

#### 1. In-Place Constraint Alterations (`[ADD CONSTRAINT]`)
When a new `PRIMARY KEY` or `UNIQUE` constraint is added to a table that previously lacked one, DuckPack applies an in-place DDL mutation without recreating the table:
```sql
ALTER TABLE users ADD PRIMARY KEY (user_id);
ALTER TABLE users ADD UNIQUE (email_address);
```
These changes are classified as non-destructive additions and are enabled by default during interactive apply.

#### 2. Atomic Copy-and-Swap Migrations (`[RECREATE TABLE]`)
DuckDB does not natively support in-place `ALTER TABLE ... ADD FOREIGN KEY`, `ADD CHECK`, or constraint drops. For these modifications, DuckPack generates an atomic, transactional copy-and-swap migration using a staging table:
```sql
BEGIN TRANSACTION;
CREATE TABLE orders__duckpack_swap (
    order_id INTEGER PRIMARY KEY,
    user_id INTEGER,
    amount DECIMAL(18,2),
    FOREIGN KEY (user_id) REFERENCES users(user_id)
);
INSERT INTO orders__duckpack_swap (order_id, user_id, amount)
    SELECT order_id, user_id, amount FROM orders;
DROP TABLE orders;
ALTER TABLE orders__duckpack_swap RENAME TO orders;
COMMIT;
```
- **Data Protection:** Because table recreation replaces the existing table structure, DuckPack classifies `[RECREATE TABLE]` as a protected operation. In interactive mode, it is deselected by default, and in automated pipelines (`duckpack apply --auto-approve`), it requires the `--force-drop` flag to proceed.
- **Rollback Safety:** If data in the existing table violates the new constraint, DuckDB automatically aborts and rolls back the transaction, keeping your existing table and data intact.

### Interactive Apply & Notebook Parity

DuckPack bridges the gap between terminal CLI power users, CI/CD pipelines, and Python data scientists:

- **CLI Workflow:** By default, `duckpack apply` opens a rich, color-coded visual confirmation screen listing every added table, altered column, and replaced view before applying changes. Press `y` to confirm, `n` / `Esc` to cancel, or `e` to jump directly into the Explore IDE. (Pass `--auto-approve` for non-interactive CI/CD runs).
- **Python Script Parity:** Calling `duckpack.apply(con, auto_approve=False)` (or `interactive=True`) brings the exact same interactive visual approval workflow to Python scripts.
- **Jupyter & VS Code Notebook Parity:** In notebook environments lacking an interactive TTY, DuckPack automatically spins up a background bridge and spawns a connected external terminal window (Ghostty, Foot, Kitty, WezTerm, Alacritty, GNOME Terminal, Konsole, Windows Terminal, or macOS Terminal.app). Developers get the full interactive confirmation UI without notebook hang-ups.

### Embedded DuckDB Version Transparency & Diagnostics

DuckPack embeds DuckDB directly as an in-process engine:

- **Storage Stability:** DuckDB has guaranteed forward and backward storage format compatibility for `.duckdb` files across minor versions following DuckDB v1.0.0.
- **Engine Transparency:** Check the exact embedded DuckDB engine version anytime via:
  ```bash
  duckpack --version
  # DuckPack v0.2.0 (embedded DuckDB v1.2.0)
  ```
  Or in Python:
  ```python
  import duckpack

  print(duckpack.__version__)         # e.g. "0.2.0"
  print(duckpack.__duckdb_version__)  # e.g. "1.2.0"
  ```
- **Proactive Storage Diagnostics:** If an older database file with an incompatible legacy storage format is encountered, DuckPack catches the internal engine error and prints clear migration guidance rather than an unhelpful internal crash dump.

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
┌ Explorer (25%) [W: Width | +/-: Resize | R: Refresh | q: Query] ──┬─ [Tab 1: cross_join.sql] ── [+ New] ── [❌ Close] ────┐
│ ▼ 📁 target_db [Active] (quack)                     │ SELECT                                                │
│   ▼ 📦 Tables (2)                                   │   ro.order_id,                                        │
│     ├─ customers (1,450 rows)                       │   ro.amount,                                          │
│     │  ├─ 🔑 customer_id (BIGINT) [PK]              │   lu.user_email                                       │
│     │  └─ email (VARCHAR)                           │ FROM target_db.orders ro                              │
│     └─ orders (8,920 rows)                          │ JOIN memory.local_users lu                            │
│        ├─ 🔑 order_id (BIGINT) [PK]                 │   ON ro.customer_id = lu.id                           │
│        └─ 🔗 customer_id (BIGINT) [FK -> cust...]   │ LIMIT 100;                                            │
│   ▼ 👁️ Views (1)                                    │                                                       │
│     └─ active_orders                                │                                                       │
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
| | `F3` / `1` | Switch Explorer to **Database Catalog** view (`target_db`, `memory`, attached DBs) |
| | `F4` / `2` | Switch Explorer to **Project Files** view (`schemas/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`, `duckpack.toml`) |
| | `F6` / `Ctrl+D` | **In-IDE Declarative Diff & Apply:** Review diff modal with `Space` toggling, dependency locks, and migration apply |
| | `Ctrl+B` / `v` | **Validate Project:** Verify SQL syntax and cross-asset dependencies against Shadow DB |
| | `Alt+W` | Cycle Explorer width presets (`25%` $\rightarrow$ `35%` $\rightarrow$ `50%`) |
| | `Alt+]` / `Alt+[` | Widen / narrow Explorer pane by 5% |
| | `Ctrl+C` / `Esc` | Cancel operation / exit modal / quit IDE |
| **Editor** | `Ctrl+E` / `F5` | **Execute query** in active tab (or execute highlighted selection) |
| | `Shift + Arrows` | Highlight text to isolate and execute *only* the selection on `Ctrl+E` |
| | `Ctrl+Space` | Trigger **Intelligent Autocomplete** (catalog-aware tables, views, columns) |
| | `Ctrl+F` | **Auto-format** SQL statement using SQL parser |
| | `Ctrl+S` | **Save** active query or project file to disk (clears dirty `*` indicator) |
| | `Ctrl+Y` / `Ctrl+C` | **Copy** selected text (or active cell/line) to system clipboard |
| | `Ctrl+V` | **Paste** from OS system clipboard into editor |
| | `Ctrl+G` / `F2` | Open **Schema Visualizer** (ASCII tree + copies Mermaid.js ERD to clipboard) |
| **Tabs** | `Ctrl+T` | Open a new blank query tab |
| | `Ctrl+W` | Close the active query tab |
| | `Ctrl+N` / `Ctrl+P` | Switch to the **Next** / **Previous** tab |
| **Explorer** | `Up` / `Down` | Navigate through databases/categories, table groups, tables, and columns |
| | `Enter` / `Space` | Expand or collapse tree folder / open SQL file in editor tab |
| | `a` / `n` | **Quick Add Asset Modal** (in Project mode: Table, View, Macro, Trigger, Schema, Query, Seed) |
| | `v` | **Validate Project** syntax and dependencies against Shadow DB (in Project mode) |
| | `W` / `w` | **Cycle width presets:** `25%` $\rightarrow$ `35%` $\rightarrow$ `50%` $\rightarrow$ `25%` |
| | `+` / `=` / `]` | **Widen Explorer** by +5% (up to 60%) |
| | `-` / `[` | **Narrow Explorer** by -5% (down to 15%) |
| | `Left` / `Right` / `h` / `l` | **Scroll horizontally** by 4 characters |
| | `Shift + Left` / `Right` | **Fast horizontal scroll** by 10 characters |
| | `Home` | **Snap horizontal scroll** back to left margin |
| | `q` / `s` | **Query table/view** (generates catalog-qualified SQL into editor) |
| | `Ctrl+A` | **Attach Database** dialog with Provider Presets (DuckDB, SQLite, Postgres, MySQL, MotherDuck, Quack) |
| | `Ctrl+K` | **Cloud Secrets & S3 Buckets:** Add S3/R2/MinIO credentials with masking, `.env` persistence, and bucket exploration |
| | `d` | **Detach Database** from current session |
| | `b` | **Cycle Target Catalog** for diff/apply operations |
| | `r` / `R` | **Refresh Explorer:** re-scans live database (in DB mode) or disk & git status (in Project mode) |
| **In-IDE Diff & Apply** | `Space` | **Toggle selection** `[x]` / `[ ]` of highlighted migration with dependency locking `[L]` |
| | `a` | **Toggle All:** select or deselect all eligible migrations |
| | `Enter` | **Apply selected changes** directly to target database catalog |
| | `Esc` | **Close** diff review dialog without applying changes |
| **Apply Review Screen** | `y` | **Confirm and apply** all pending declarative migrations |
| | `n` / `Esc` | **Abort and cancel** migrations without modifying database |
| | `e` | **Explore IDE:** launch full terminal query IDE on target database |
| **Results Grid** | `Arrows` | Scroll vertically through rows and horizontally through columns |
| | `Shift + ◄ / ►` | Fast horizontal scroll across wide result tables |
| | `Home` / `End` | Jump directly to the **First** / **Last** column |
| | `PgUp` / `PgDn` | Fast scroll vertically |
| | `n` / `p` | Navigate to **Next** / **Previous** page (500-row pagination chunks) |
| | `Ctrl+O` | Export active query results to a CSV file in `queries/` |
| **Mouse Support** | Mouse Wheel | Vertical & horizontal (tilt or `Shift+Wheel`) scrolling in focused pane |
| | Click Tabs | Click on tab headers to switch or click `[+ New]` / `[❌ Close]` buttons |
| | Click Explorer Header | Click `[F3: DB]` or `[F4: Project]` in header to switch modes, or click `[+ Attach]` / `[+ New]` |

### Project Authoring Mode & In-IDE Declarative Workflow (`F4`)

DuckPack Explore features a dual-mode sidebar that brings the complete declarative workflow directly into your terminal workspace:

- **Mode Switching (`F3` / `1` vs `F4` / `2`):**
  - **`[F3: Database Catalog]`**: Inspect live target and attached database catalogs, schemas, tables, views, triggers, columns, and constraints (`🔑`, `🔗`).
  - **`[F4: Project Files]`**: Browse declarative asset files organized categorically (`📁 Schemas`, `📦 Tables`, `👁️ Views`, `🔧 Macros`, `⚡ Triggers`, `📄 Queries`, `🌱 Seeds`, `⚙️ Config`).
- **Complete Scaffolding & Zero-Exit Authoring:** All canonical project categories (`schemas/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`) are always rendered in the project sidebar tree—even if the underlying folder does not exist on disk yet. Empty categories show an interactive `(empty - press 'a' to add)` placeholder so developers never need to leave DuckPack to scaffold new directories or assets.
- **Passive Git Awareness:** The Explorer header dynamically shows the current branch (e.g. `Project (main*) (25%)`), and files display status badges (`[M]` modified, `[+]` untracked, `[D]` deleted). DuckPack never commits or pushes, preserving index safety and SSH autonomy.
- **Guided Asset Scaffolding (`a` or `n`):** Quick Add modal to scaffold a new Table, View, Macro, Trigger, Schema, Query, or Seed with clean boilerplate SQL/CSV. It automatically detects the selected folder category, creates the directory on disk if needed, writes the boilerplate, and opens the file in a new editor tab.
- **Direct File Editing & Dirty Tab Tracking:** Opened files bind directly to disk. Tabs show `*` when dirty (`users.sql *`). Pressing `Ctrl+S` writes changes to disk, clears the dirty indicator, and refreshes project file badges.
- **In-IDE Syntax Validation (`Ctrl+B` or `v`):** Compiles the entire project against an in-memory Shadow DB to verify SQL syntax, macros, and cross-asset dependencies with error line jumps.
- **In-IDE Diff & Selective Apply (`F6` or `Ctrl+D`):** Computes declarative diffs against the active target database and presents an interactive diff modal directly inside the IDE. Supports `Space` to select/deselect individual changes with automatic dependency locking (`[L]`) and applies only the selected migrations on `Enter`.

### Primary Key & Foreign Key Visual Identifiers

The Explorer sidebar introspects constraints from `duckdb_constraints()` for both local DuckDB databases and remote Quack instances, rendering relational relationships directly beside column names:

- **Primary Key (`🔑`)**:
  ```text
  🔑 customer_id (BIGINT) [PK]
  ```
- **Foreign Key (`🔗`)**: Explicitly indicates the referenced table and column target:
  ```text
  🔗 user_id (INTEGER) [FK -> users.user_id]
  ```
- **Composite Primary Key + Foreign Key (`🔑🔗`)**: When a column serves as both a primary key and a foreign key:
  ```text
  🔑🔗 tenant_id (BIGINT) [PK, FK -> tenants.id]
  ```

### Dynamic Explorer Width Resizing & Snappy Scrolling

When inspecting tables with long column names, complex data types (`MAP(VARCHAR, JSON)`), or nested catalog schemas, standard sidebar widths can feel cramped:

- **Width Presets (`W` or `Alt+W`):** Instantly cycle the Explorer width between **`25%`**, **`35%`**, and **`50%`** without leaving the keyboard.
- **Fine Adjustment (`+` / `-` or `[` / `]`):** Fine-tune sidebar width in 5% increments between 15% and 60% (e.g. `Alt+]` or `Alt+[` from any pane).
- **Responsive Header:** The Explorer title updates dynamically with the active percentage and shortcut hints:
  ```text
  Explorer (35%) [W: Width | +/-: Resize | R: Refresh | q: Query]
  ```
- **Snappy Horizontal Scrolling:**
  - `Left` / `Right` (or `h` / `l`): Shifts by 4 characters.
  - `Shift + Left` / `Shift + Right`: Jumps by 10 characters.
  - `Home`: Instantly snaps back to column 0.
  - Mouse wheel tilt or `Shift + Wheel`: Smooth horizontal scrolling across long metadata strings.

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

### Live Database Attachment with Provider Presets (`Ctrl+A` / `d`)

Press **`Ctrl+A`** from anywhere in the IDE to open the interactive **Attach Database** modal dialog with field-adaptive provider presets:

- **Provider Presets (cycle via `◄` / `►` or `h` / `l`):**
  - **`DuckDB (Local / In-Memory)`**: Attach `.duckdb` files or `:memory:` scratchpads.
  - **`Remote Quack Server`**: Connect to live remote Quack server instances with optional token credentials.
  - **`MotherDuck Cloud`**: Attach MotherDuck cloud databases (`md:database_name`) with optional MotherDuck service token injection.
  - **`SQLite`**: Attach SQLite database files (`.db`, `.sqlite`) with automatic installation and loading of the DuckDB `sqlite` extension (`INSTALL sqlite; LOAD sqlite;`).
  - **`PostgreSQL`**: Connect to remote Postgres servers via connection URI (`postgres://user:pass@host:5432/dbname`) with automatic extension handling.
  - **`MySQL`**: Connect to remote MySQL servers via connection URI (`mysql://user:pass@host:3306/dbname`) with automatic extension handling.
- **Dynamic Field Guidance**: Prompts, placeholders, and catalog alias hints dynamically adapt to the selected provider.
- **Masked Token/Password Field**: Passwords and service tokens are rendered with `*` mask characters.
- **Read-Only Toggle**: Press `Space` to toggle `READ_ONLY` mode.
- **Safe Detach**: Select any attached database in the Explorer and press **`d`** (or `Delete`) to safely detach it from the active session.

### Cloud Secrets Management & S3 Bucket Explorer (`Ctrl+K`)

DuckPack provides complete cloud data lake support directly in your terminal without requiring you to remember low-level `CREATE SECRET` SQL syntax:

- **Dedicated "Secrets & Cloud Buckets" Sidebar Drawer (`🔐 Secrets & Cloud`):**
  - Displays all active secrets from `duckdb_secrets()` (e.g. `🔑 s3_secret [s3]`).
  - Lists tracked cloud buckets (`🪣 s3://analytics-lake`).
  - Displays quick action `➕ Add Cloud Secret (Ctrl+K)`.
- **Live S3 Bucket Explorer:**
  - Expanding a bucket node runs a background DuckDB `glob('{bucket}/*')` query to list remote files and directories.
  - Formatted file icons identify `.parquet` (📊), `.csv` (📝), and general files (📄).
  - Pressing **`Enter`**, **`q`**, or **`s`** on any cloud file instantly loads an optimized query into the editor tab:
    ```sql
    SELECT * FROM read_parquet('s3://my-bucket/events/2026/01/data.parquet') LIMIT 500;
    ```
- **Cloud Secrets Modal (`Ctrl+K` or Explorer `a`/`n`):**
  - **Provider Presets**: Cycle between **AWS S3**, **Cloudflare R2**, **MinIO**, **Wasabi**, and **Custom S3 Endpoint**.
  - **Visual Masking**: Secret access keys are visually masked with `*` to protect against screen-sharing leaks.
  - **Dual Persistence Architecture**:
    - **DuckDB Persistent Secret**: Creates persistent secrets in DuckDB via `CREATE OR REPLACE PERSISTENT SECRET` (with fallback to session secrets).
    - **Project `.env` Persistence**: Optionally appends or updates `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT`, and `DUCKPACK_S3_BUCKET` in the project `.env` file (strictly git-ignored).
  - **Automatic Extension Installation**: Transparently executes `INSTALL httpfs; LOAD httpfs;` so remote operations never fail.

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

# Option A: Zero arguments (auto-resolves target, endpoint, and credentials from duckpack.toml / .env)
duckpack.explore()

# Option B: Pass live in-memory connection (auto-attaches external databases from duckpack.toml)
duckpack.explore(con)

# Option C: Switch environment profile directly in Python
duckpack.explore(env="staging")

# Optional controls:
# duckpack.explore(con, open_window=False)  # Force in-cell formatted overview (headless mode)
# duckpack.explore(con, wait=False)         # Run terminal asynchronously in background
```

---

### 2. Declarative Migrations in Pytest & Pipelines (`apply`)

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

For interactive review before applying changes in your Python scripts or Jupyter notebooks, set `auto_approve=False` (or `interactive=True`). In Jupyter or VS Code, DuckPack automatically launches a connected native terminal window:

```python
# Launch interactive TUI diff review screen before applying:
result = duckpack.apply(con, project_dir="./duckdb_project", auto_approve=False)
```

---

### 3. Pipeline Schema Drift Detection (`diff`)

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

### 4. Multi-Database Catalog Targeting in Python

When working with DuckDB connections that attach multiple databases, use the `database` (or `catalog`) parameter to isolate migrations and drift checks to that specific catalog:

```python
import duckdb
import duckpack

con = duckdb.connect()

# Attach an external analytics database
con.execute("ATTACH 'analytics.duckdb' AS analytics;")

# Target only 'analytics' (ignores tables in default/memory catalog)
diff = duckpack.diff(con, project_dir="./analytics_repo", database="analytics")

if diff.has_changes:
    print(f"Pending changes for analytics: {diff.change_count}")

# Interactively review diff and apply to 'analytics'
# (Spawns a connected terminal pop-out in Jupyter / VS Code)
result = duckpack.apply(
    con,
    project_dir="./analytics_repo",
    database="analytics",
    auto_approve=False  # or interactive=True
)
```

---

### 5. Python API Reference

```python
import duckpack
```

#### Engine Version Properties
- **`duckpack.__version__: str`**: The version of DuckPack (e.g. `"0.2.0"`).
- **`duckpack.__duckdb_version__: str`**: The embedded DuckDB engine version (e.g. `"1.2.0"`).

#### Functions

- **`duckpack.explore(target=None, project_dir=".", quack_token=None, insecure=False, open_window=True, wait=True, database=None, catalog=None, env=None) -> None`**
  Launches the interactive TUI IDE against a live connection (`duckdb.DuckDBPyConnection`), database URI (`str`), or `None` (auto-resolves from `duckpack.toml` and `.env`). When passing an existing in-memory connection `con`, automatically attaches configured external databases from `duckpack.toml`. In Jupyter Notebooks or VS Code Dev Containers, renders an interactive card with a 1-click **`▶ Open DuckPack in VS Code Terminal`** action and live background bridge. Pass `env` to switch environment profiles (e.g. `env="staging"`).

- **`duckpack.stop_server() -> bool`**
  Stops any active background Quack session bridge started by `duckpack.explore(con)` in a notebook.

- **`duckpack.apply(target=None, project_dir=".", auto_approve=True, force_drop=False, interactive=False, database=None, catalog=None, env=None) -> ApplyResult`**
  Computes schema diffs and applies declarative migrations. In interactive mode (`auto_approve=False` or `interactive=True`), opens the visual review TUI. When `target=None`, auto-resolves database targets from `duckpack.toml`. Returns an `ApplyResult`.

- **`duckpack.diff(target=None, project_dir=".", database=None, catalog=None, env=None) -> DiffSummary`**
  Compares declarative project state against target database or attached catalog without mutating data. When `target=None`, auto-resolves from `duckpack.toml`. Returns a `DiffSummary`.

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

### DuckDB v2.0 Native `CONNECT` Session Management

In DuckDB v2.0, the Quack remote protocol graduates to first-class connection sessions via the native `CONNECT target_db;` statement. When DuckPack attaches to a remote Quack server, it immediately establishes session context on the remote server:

```sql
ATTACH 'quack:analytics.company.com:443' AS target_db;
CONNECT target_db;
```

With `CONNECT target_db;` active:
- **Zero-Friction Execution:** All standard queries, DDL (`CREATE TABLE`, `CREATE SCHEMA`), DML (`INSERT`, `UPDATE`, `DELETE`), CTEs, and session configuration commands execute directly on the remote server without manual wrappers.
- **Native Secrets & Settings:** `CREATE SECRET (TYPE S3 ...)` and `SET variable = ...` statements apply directly to the remote server session without needing special client-side redirection.

### The `-- @remote` Query Directive (Sunset & Backwards Compatibility)

In DuckDB 1.x, session-scoped entities (such as secrets and configuration settings) were held in local client memory unless wrapped via `-- @remote`. 

With DuckDB v2.0's native `CONNECT` architecture, **`-- @remote` is sunsetted as a requirement**. DuckPack maintains backwards-compatible support: if an existing script or query contains `-- @remote`, DuckPack strips the directive and executes the statement natively without errors.

| Statement Type | DuckDB v1.x (`-- @remote`) | DuckDB v2.0 (`CONNECT target_db`) |
| :--- | :---: | :--- |
| `CREATE TABLE` / `CREATE VIEW` / `CREATE SECURE VIEW` | Standard SQL | Native server-side execution. |
| `SELECT` / CTEs / DML | Standard SQL | Native server-side execution (including CTE DML). |
| `CREATE SECRET (TYPE S3 ...)` | Required `-- @remote` | Native server-side execution via `CONNECT`. |
| `SET variable = ...` | Required `-- @remote` | Native server-side execution via `CONNECT`. |
| `INSTALL` / `LOAD` | Required `-- @remote` | Native server-side execution via `CONNECT`. |


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
├── schemas/               # CREATE SCHEMA statements (supports nested schemas e.g. finance/reports.sql)
├── tables/                # CREATE TABLE statements (one table per .sql file; VARIANT types supported)
├── views/                 # CREATE VIEW & CREATE SECURE VIEW statements (engine auto-forces OR REPLACE)
├── macros/                # CREATE MACRO statements (supports DuckDB 2.0 lambda x: ... syntax)
├── triggers/              # CREATE TRIGGER statements (auto-preserved during table copy-and-swap)
├── queries/               # Scratchpad queries, exports, and query history
├── seeds/                 # Static CSVs or data seed files
├── scripts/
│   ├── pre-deploy/        # Raw SQL executed BEFORE migrations are applied
│   └── post-deploy/       # Raw SQL executed AFTER migrations are applied
├── duckpack.toml          # Multi-environment targets configuration
├── .env                   # Environment variables for ${VAR} template substitution
└── .duckdbignore          # Ignore patterns for tables/views
```

### DuckDB v2.0 Authoring Guidelines

- **Arbitrary Nested Schemas:** Schemas can be nested to any depth (`CREATE SCHEMA finance.reports;`). Table files can live in nested subdirectories (`tables/finance/reports/q3.sql`) or dot-named files (`tables/finance.reports.q3.sql`). DuckPack automatically resolves topological creation order (parents before children) and teardown order (children before parents).
- **First-Class `VARIANT` Columns:** Tables and views support native `VARIANT` types and `variant_*` functions alongside JSON casts (`::JSON::VARIANT`). In-place type alterations (`ALTER TABLE t ALTER c TYPE VARIANT`) are applied without requiring table recreation.
- **`CREATE SECURE VIEW`:** Secure views are fully supported in `views/`. When views are modified, DuckPack automatically emits `CREATE OR REPLACE SECURE VIEW` to preserve view protection.
- **Double-Arrow / `lambda` Macro Syntax:** In DuckDB 2.0, single-arrow lambdas (`x -> x + 1`) are deprecated. Write macros using `lambda x: x + 1` or double-arrow syntax:
  ```sql
  CREATE MACRO core.double_all(arr) AS list_transform(arr, lambda x: x * 2);
  ```
- **Automatic Trigger Preservation:** DuckDB 2.0 cascades table drops to all attached triggers. DuckPack's declarative diff engine detects when a table requires a copy-and-swap migration and automatically re-emits `CREATE TRIGGER` statements immediately after the table swap, preventing trigger loss.


### Configuration (`duckpack.toml`)

Define connection targets, remote Quack instances, and environment profiles without hardcoding sensitive credentials or database paths:

```toml
[project]
name = "analytics_warehouse"
version = "0.2.0"

[target]
default_db = "dev.duckdb"
database = "analytics"     # Optional: default attached catalog name to target

# Or configure a default remote Quack target with structured host/port:
# host = "analytics.internal"
# port = 9494
# insecure = false

[env.dev]
db = "dev.duckdb"

[env.staging]
# Connect directly to a remote Quack server instance:
db = "quack:staging.company.internal:9494"
database = "analytics_stg"
insecure = true            # Disable SSL for internal/self-signed deployments

[env.prod]
# Connect to production Quack or MotherDuck:
db = "quack:analytics.company.com:443"
database = "analytics"
token_env = "PROD_QUACK_TOKEN" # Look up token from PROD_QUACK_TOKEN in .env
```

Deploy, diff, or explore against any profile:
```bash
duckpack explore                   # Automatically connects using [target] settings
duckpack explore --env staging     # Connects to staging Quack server with no flags needed
duckpack apply --env prod          # Applies migrations to prod with secrets loaded from .env
```

### Environment Variables & Credentials (`.env`)

#### 1. Credential Safety (Zero Secrets in Shell History)
Never pass raw API tokens on the command line where they are saved into `.bash_history` or `/proc/<pid>/cmdline`. DuckPack automatically loads `.env` (or `.env.<profile>` when using `--env <profile>`) and checks for:
- `QUACK_TOKEN`: Default authentication token for remote Quack servers.
- `DUCKDB_QUACK_TOKEN`: Standard DuckDB Quack token fallback.
- Custom environment variable name defined via `token_env = "MY_TOKEN"` in `duckpack.toml`.

```bash
# .env (automatically ignored by git)
QUACK_TOKEN=your_secure_quack_token_here
PROD_QUACK_TOKEN=production_super_secret_token
```

#### 2. SQL Template Interpolation
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
