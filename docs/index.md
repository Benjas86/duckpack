# DuckPack Documentation

Welcome to the DuckPack documentation hub. DuckPack brings **declarative schema management ("SSDT for DuckDB")**, a **full-featured Terminal IDE**, **multi-catalog workspace orchestration**, and **first-class Python & Notebook bindings** to the DuckDB v2.0 ecosystem.

---

## Documentation Map

| Guide | What You'll Learn |
| :--- | :--- |
| **[Getting Started](./getting-started.md)** | Install the CLI binary, Python package (`pip`/`uv`), or Cargo crate, and run your first 5-minute declarative workflow (`init` → `build` → `diff` → `apply` → `explore`). |
| **[Declarative Projects & Authoring](./declarative-projects.md)** | Project layout (`schemas/`, `sequences/`, `tables/`, `views/`, `macros/`, `triggers/`, `seeds/`, `scripts/`), DuckDB v2.0 SQL features, constraint diffing & copy-and-swap, smart renames, `.env` templating, and `.duckpackignore`. |
| **[Terminal IDE Guide](./terminal-ide.md)** | Dual-mode Explorer (`F3: DB` vs. `F4: Project`), full Keyboard Shortcuts Cheat Sheet, PK/FK badges, width resizing, `F2` per-tab DB switcher, `Ctrl+A` Attach presets, `Ctrl+K` Cloud Secrets, `Ctrl+G` Mermaid ERDs, and CSV export. |
| **[Workspace & Configuration (`duckpack.toml`)](./workspace-and-configuration.md)** | Multi-environment `[target]` / `[env.<name>]`, declarative `[[secrets]]` (`*_env` hygiene), multi-catalog `[[attach]]`, in-memory Workspace Mode, resilient startup placeholders, and `Ctrl+S` TUI round-trip persistence. |
| **[DuckLake Lakehouse Guide](./ducklake.md)** | Connection Triad (DuckDB File, PostgreSQL, SQLite, Quack + `DATA_PATH` + frozen remote URLs), metadata-only schema evolution, `--allow-lake-rewrite` guard, `set_commit_message` snapshot tagging, and interactive Time-Travel (`t`). |
| **[Quack Remote Server & RBAC](./quack-and-rbac.md)** | Dual-catalog `target_db` + `memory` sessions, cross-DB joins, auto-routing & `-- @remote`, SSL/TLS options, declarative `[quack_auth]` RBAC, `Ctrl+U` Live Session Monitor, and `--rbac-only` post-start hooks (Docker, Compose, K8s, systemd). |
| **[External Databases & Cloud Storage](./external-databases.md)** | Connecting MSSQL (native TDS, Windows Auth/Kerberos, named instances, `mssql://` URIs), PostgreSQL, MySQL, SQLite, MotherDuck, and S3 / R2 / GCS / Azure / MinIO buckets, plus the SSDT vs. Federation boundary. |
| **[Python & Notebook Integration](./python-and-notebooks.md)** | In-memory `duckpack.explore(con)`, Jupyter/VS Code pop-out terminals & `SessionBridge` two-way sync, Pytest migration fixtures (`apply`), pipeline drift checks (`diff`), multi-catalog targeting, and the complete Python API reference. |
| **[CI/CD & Production Deployment](./cicd-and-deployment.md)** | Immutable `.duckpack` snapshot artifacts (`compile`), non-interactive `--auto-approve`, dry-run SQL generation (`--out`), SSH `deploy` & `explore --remote`, and GitHub Actions recipes. |
| **[CLI Reference](./cli-reference.md)** | Exhaustive reference for every `duckpack` subcommand, flag, environment variable, and embedded DuckDB version diagnostic. |
| **[Architecture: How DuckPack Works](./architecture.md)** | Under the hood: Shadow DB topological compilation, `DbExecutor` seam, atomic copy-and-swap, Quack `SessionBridge`, and DuckDB v2.0 storage compatibility. |

---

## Suggested Learning Paths

### 1. Local DuckDB & Declarative Migrations
1. [Getting Started](./getting-started.md)
2. [Declarative Projects & Authoring](./declarative-projects.md)
3. [Terminal IDE Guide](./terminal-ide.md)
4. [CI/CD & Production Deployment](./cicd-and-deployment.md)

### 2. Python Pipelines, Pytest & Jupyter Notebooks
1. [Getting Started](./getting-started.md#2-python-package-pip--uv)
2. [Python & Notebook Integration](./python-and-notebooks.md)
3. [Workspace & Configuration](./workspace-and-configuration.md)

### 3. Multi-Container DuckLake, Quack & Enterprise Federation
1. [Workspace & Configuration](./workspace-and-configuration.md)
2. [DuckLake Lakehouse Guide](./ducklake.md)
3. [Quack Remote Server & RBAC](./quack-and-rbac.md)
4. [External Databases & Cloud Storage](./external-databases.md)
