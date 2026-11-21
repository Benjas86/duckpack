[← Docs Index](./index.md)

# Terminal IDE Guide

DuckPack ships a built-in, zero-dependency Terminal IDE (built with Ratatui & Crossterm) for live database exploration, multi-catalog querying, cloud bucket browsing, DuckLake time travel, Quack RBAC management, and in-IDE declarative authoring.

```bash
# Explore a local database
duckpack explore --db local.duckdb

# Explore a multi-catalog Workspace defined via [[attach]] in duckpack.toml
duckpack explore

# Explore a specific environment profile
duckpack explore --env staging

# Or jump directly from `duckpack apply` by pressing 'e'
```

```text
┌ Explorer (25%) [W: Width | +/-: Resize | R: Refresh | q: Query] ──┬─ [Tab 1: cross_join.sql] ── [+ New] ── [❌ Close] ────┐
│ ▼ 📁 target_db [Active] (quack)                                    │ SELECT                                                │
│   ▼ 📦 Tables (2)                                                  │   ro.order_id,                                        │
│     ├─ customers (1,450 rows)                                      │   ro.amount,                                          │
│     │  ├─ 🔑 customer_id (BIGINT) [PK]                             │   lu.user_email                                       │
│     │  └─ email (VARCHAR)                                          │ FROM target_db.orders ro                              │
│     └─ orders (8,920 rows)                                         │ JOIN memory.local_users lu                            │
│        ├─ 🔑 order_id (BIGINT) [PK]                                │   ON ro.customer_id = lu.id                           │
│        └─ 🔗 customer_id (BIGINT) [FK -> cust...]                  │ LIMIT 100;                                            │
│   ▼ 👁️ Views (1)                                                   │                                                       │
│     └─ active_orders                                               │                                                       │
│ ▼ 📁 memory (duckdb)                                               │                                                       │
│   ▼ 📦 Tables (1)                                                  │                                                       │
│     └─ local_users (50 rows)                                       │                                                       │
├────────────────────────────────────────────────────────────────────┴───────────────────────────────────────────────────────┤
│ Query Results [Page 1/1] (Showing 50 rows)                                                                                 │
│ order_id │ amount  │ user_email                                                                                            │
│ 1001     │ 149.99  │ alice@example.com                                                                                     │
│ 1002     │ 89.50   │ bob@example.com                                                                                       │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## Keyboard Shortcuts Cheat Sheet

| Scope | Shortcut | Action |
| :--- | :--- | :--- |
| **Global / Navigation** | `Tab` | Cycle focus between **Explorer**, **Editor**, and **Results** panes |
| | `?` | Toggle the built-in Help Modal |
| | `F3` / `1` | Switch Explorer to **Database Catalog** view (`target_db`, `memory`, attached DBs) |
| | `F4` / `2` | Switch Explorer to **Project Files** view (`schemas/`, `sequences/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`, `duckpack.toml`) |
| | `F6` / `Ctrl+D` | **In-IDE Declarative Diff & Apply** modal with `Space` selection & dependency locks |
| | `Ctrl+B` / `v` | **Validate Project** SQL syntax and dependencies against Shadow DB |
| | `Alt+W` | Cycle Explorer width presets (`25%` → `35%` → `50%`) |
| | `Alt+]` / `Alt+[` | Widen / narrow Explorer pane by 5% |
| | `Ctrl+C` / `Esc` | Cancel modal / confirm quit IDE |
| **Editor** | `Ctrl+E` / `F5` | **Execute query** in active tab (or execute highlighted selection only) |
| | `Shift + Arrows` | Highlight a sub-statement to run only the selected SQL on `Ctrl+E` |
| | `F2` | **Database Quick-Picker:** switch active tab's target catalog (`▲`/`▼` or `j`/`k`, `Enter`) |
| | `Ctrl+Space` | **Intelligent Autocomplete** (prioritizes active tab's catalog objects & columns) |
| | `Ctrl+F` | **Auto-format** SQL statement |
| | `Ctrl+S` | **Save** active query or project file to disk (clears dirty `*` badge) |
| | `Ctrl+Y` / `Ctrl+C` | **Copy** selection to OS system clipboard |
| | `Ctrl+V` | **Paste** from OS system clipboard |
| | `Ctrl+G` | **Schema Visualizer:** ASCII relationship tree + copies Mermaid.js ERD to clipboard |
| **Tabs** | `Ctrl+T` | Open new blank query tab (inherits active tab's target database) |
| | `Ctrl+W` | Close active query tab |
| | `Ctrl+N` / `Ctrl+P` | Switch to **Next** / **Previous** tab |
| **Explorer** | `Up` / `Down` | Navigate catalogs, groups, tables, columns, snapshots, buckets, and project files |
| | `Enter` / `Space` | Expand/collapse folder, open SQL file, or retry offline workspace placeholder |
| | `a` / `n` | **Quick Add Asset Modal** (Project mode: Table, View, Macro, Trigger, Sequence, Schema, Query, Seed) |
| | `W` / `w` | Cycle Explorer width presets (`25%` → `35%` → `50%` → `25%`) |
| | `+` / `=` / `]` | Widen Explorer by +5% (up to 60%) |
| | `-` / `[` | Narrow Explorer by -5% (down to 15%) |
| | `Left` / `Right` / `h` / `l` | Scroll Explorer horizontally (4 chars) |
| | `Shift + Left` / `Right` | Fast horizontal scroll (10 chars) |
| | `Home` | Snap horizontal scroll back to column 0 |
| | `q` / `s` | Generate catalog-qualified `SELECT` query into editor (or query a DuckLake snapshot) |
| | `t` | **DuckLake Time-Travel Modal:** `AT (VERSION => N)`, `AT (TIMESTAMP => TIMESTAMPTZ '...')`, `ducklake_table_changes(...)` |
| | `Ctrl+A` | **Attach Database** modal with Provider Presets & optional `Ctrl+S` save to `duckpack.toml` |
| | `Ctrl+K` | **Cloud Secrets & S3 Buckets** modal with masked keys & optional `Ctrl+S` save to `duckpack.toml` |
| | `Ctrl+U` | **Quack RBAC & Live Session Monitor** (`[`/`]` cycles attached Quack & DuckLake-on-Quack servers) |
| | `d` | **Detach** selected database from session |
| | `b` | **Cycle Target Catalog** for diff/apply operations |
| | `r` / `R` | **Refresh Explorer** (also invalidates MSSQL/Postgres/MySQL/MotherDuck/Quack catalog caches & retries failed workspace nodes) |
| **In-IDE Diff & Apply** | `Space` | Toggle item `[x]` / `[ ]` with automatic dependency locking `[L]` |
| | `a` | Toggle all eligible items |
| | `w` | Toggle **Allow Lake Rewrite** (`[OFF/ON]`) for DuckLake copy-and-swap rewrites |
| | `Enter` | Apply selected changes directly |
| | `Esc` | Close diff review without applying |
| **Results Grid** | `Arrows` | Scroll rows vertically and columns horizontally |
| | `Shift + ◄ / ►` | Fast horizontal column scrolling across wide tables |
| | `Home` / `End` | Jump to first / last column |
| | `PgUp` / `PgDn` | Fast vertical page scroll |
| | `n` / `p` | Next / Previous 500-row result page |
| | `Ctrl+O` | Export active query results to CSV in `queries/` |
| **Mouse** | Wheel / `Shift+Wheel` | Vertical and horizontal scrolling in focused pane |
| | Click | Switch tabs, click `[+ New]` / `[❌ Close]`, toggle `[F3: DB]` / `[F4: Project]`, click `[+ Attach]` |

---

## Key IDE Workflows

### Dual-Mode Explorer: Live Catalog (`F3`) vs. Project Authoring (`F4`)

- **`[F3: DB]` (Database Catalog Mode):** Browse every attached database (`📁` DuckDB, `🌊` DuckLake, `🔌` MSSQL/Postgres/MySQL/SQLite, remote Quack `target_db`), including `[temp]` session tables/views, `📸 Snapshots (N)` for DuckLake, `🔐 Secrets & Cloud`, and `🛡️ Quack RBAC & Sessions (Ctrl+U)`.
- **`[F4: Project]` (Project Authoring Mode):** Browse and edit declarative SQL files across all canonical categories (`schemas/`, `sequences/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`, `duckpack.toml`).
  - **Passive Git Badges:** Header displays current branch (e.g. `Project (main*)`), and files show `[M]`, `[+]`, `[D]` status without DuckPack ever touching your Git index.
  - **Guided Scaffolding (`a`/`n`):** Creates missing directories automatically, writes boilerplate SQL/CSV, and opens the new file in a tab.
  - **Live Validation (`Ctrl+B`/`v`) & Diff/Apply (`F6`/`Ctrl+D`):** Validate against Shadow DB and selectively apply changes without leaving the IDE.

### Visual PK / FK Indicators & Data Type Colouring

Columns in the Explorer tree introspect `duckdb_constraints()` (scoped strictly per catalog and schema):

```text
🔑 customer_id (BIGINT) [PK]
🔗 user_id (INTEGER) [FK -> users.user_id]
🔑🔗 tenant_id (BIGINT) [PK, FK -> tenants.id]
```

### Per-Tab Database Context (`F2`)

Each editor tab tracks its own target database context independently:
- The tab strip shows the active catalog badge (`report.sql (sales_lake) * | scratch.sql (memory)`).
- Switching tabs automatically runs `USE "<catalog>"` and scopes `Ctrl+Space` autocomplete suggestions to that catalog first.
- Press **`F2`** anytime to switch the current tab's database context while keeping full cross-database `JOIN` capability.

### Live External Catalog Refresh (`r`)

Pressing **`r`** in Database mode (or executing DDL) refreshes cached metadata automatically for all attached external engines:
- **Quack:** `CALL quack_clear_cache();` (in-place refresh preserving per-alias `TOKEN`/`CLIENT_ID`)
- **MSSQL:** `SELECT mssql_invalidate_cache('<alias>');` + `SELECT mssql_refresh_cache('<alias>');`
- **PostgreSQL:** `CALL postgres_clear_cache();`
- **MySQL:** `CALL mysql_clear_cache();`
- **MotherDuck:** `REFRESH DATABASES;`

### Resilient Workspace Placeholders

When launching with declarative `[[attach]]` entries in `duckpack.toml`, offline containers or missing `*_env` credentials never crash startup — they appear directly in Explorer as actionable nodes:
- `⚠️ sales_lake (offline — Enter/r to retry)`
- `🔒 hr_lake (missing QUACK_HR_TOKEN)`

Press **`Enter`** or **`r`** on any placeholder node once the container is up or `.env` is updated to mount it live.
