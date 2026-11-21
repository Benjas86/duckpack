[← Docs Index](./index.md)

# Python & Notebook Integration

DuckPack ships native PyO3 Python bindings (`pip install duckpack` / `uv add duckpack`) with pre-compiled `abi3` wheels for Python >= 3.10 across Linux, macOS, and Windows.

---

## 1. Interactive In-Memory Debugging (`explore`)

Drop straight into the full Terminal IDE inside any Python script, ETL pipeline, or `breakpoint()` / `pdb` session:

```python
import duckdb
import duckpack

con = duckdb.connect()
con.execute("CREATE TABLE users AS SELECT 1 AS id, 'Alice' AS name;")
con.execute("CREATE TEMP TABLE staging AS SELECT range AS id, random() AS metric FROM range(1000);")

# Launches the full interactive Terminal IDE connected to your live in-memory session.
# Temporary tables appear with a [temp] badge and sync two-way back to `con`!
duckpack.explore(con)

# When you quit the TUI (Esc / q), Python resumes right here.
print("Resuming pipeline...")
```

### Zero-Argument Workspace & Environment Profiles

```python
import duckpack

# Auto-resolves [target] or [[attach]] Workspace Mode from duckpack.toml & .env
duckpack.explore()

# Switch environment profile directly from Python
duckpack.explore(env="staging")

# Pass an existing connection — DuckPack auto-attaches configured external targets from duckpack.toml
duckpack.explore(con, project_dir="./warehouse", env="dev")
```

---

## 2. Jupyter Notebooks & VS Code Support

Terminal UIs normally crash inside Jupyter cells (`os error 6: No such device or address`) because notebooks lack a TTY. DuckPack solves this automatically via **`SessionBridge` + `InteractionRunner`**:

- Spins up an ephemeral localhost Quack bridge bound to your live in-memory `duckdb.DuckDBPyConnection` (including temporary tables).
- Automatically spawns a native terminal window (**Ghostty, Foot, Kitty, WezTerm, Alacritty, GNOME Terminal, Konsole, Windows Terminal, macOS Terminal.app / iTerm2**) connected to your live notebook data.
- In headless environments or VS Code Dev Containers without a desktop window manager, renders an informative notebook card with a 1-click terminal launch command or falls back gracefully to a formatted schema summary.

```python
# Optional notebook controls:
duckpack.explore(con, open_window=False)  # Force in-cell formatted overview (headless mode)
duckpack.explore(con, wait=False)         # Launch terminal window asynchronously in background

# Manually stop a background SessionBridge if launched with wait=False:
duckpack.stop_server()
```

---

## 3. Declarative Migrations in Pytest & ETL Pipelines (`apply`)

Use your single declarative SQL project to initialize fresh in-memory test databases in `pytest`:

```python
import duckdb
import duckpack
import pytest


@pytest.fixture
def db():
    con = duckdb.connect()
    result = duckpack.apply(con, project_dir="./my_warehouse", auto_approve=True)
    assert result.success
    yield con
    con.close()


def test_insert_customer(db):
    db.execute("INSERT INTO core.customers (customer_id, email) VALUES (1, 'bob@example.com');")
    assert db.execute("SELECT count(*) FROM core.customers;").fetchone()[0] == 1
```

### Interactive Review from Python & Notebooks

Set `auto_approve=False` (or `interactive=True`) to launch the exact same color-coded diff review TUI before applying migrations (automatically popping out a terminal window in Jupyter/VS Code):

```python
result = duckpack.apply(con, project_dir="./my_warehouse", auto_approve=False)
print(f"Success: {result.success}, Applied: {result.applied_count}")
```

---

## 4. Pipeline Schema Drift Detection (`diff`)

Fail fast in ETL/ELT jobs when a target database drifts from your Git-tracked schema:

```python
import duckdb
import duckpack

con = duckdb.connect("production.duckdb")
summary = duckpack.diff(con, project_dir="./my_warehouse")

if summary.has_changes:
    print(f"Schema drift detected ({summary.change_count} changes):")
    for item in summary.items:
        print(f" - {item}")
    raise RuntimeError("Target database schema does not match codebase!")
```

---

## 5. Multi-Database Catalog Targeting

Isolate `diff` and `apply` to a single attached catalog using `database=` (or its alias `catalog=`):

```python
import duckdb
import duckpack

con = duckdb.connect()
con.execute("ATTACH 'analytics.duckdb' AS analytics;")

# Only compares and migrates objects inside the 'analytics' catalog
summary = duckpack.diff(con, project_dir="./analytics_repo", database="analytics")
if summary.has_changes:
    duckpack.apply(
        con,
        project_dir="./analytics_repo",
        database="analytics",
        auto_approve=True,
    )
```

---

## 6. Python API Reference

```python
import duckpack
```

### Version Properties
- `duckpack.__version__: str` — Installed DuckPack package version (e.g. `"0.3.5"`).
- `duckpack.__duckdb_version__: str` — Embedded DuckDB engine version (e.g. `"2.0.0"`).

### Functions

| Function | Signature & Behavior |
| :--- | :--- |
| **`explore`** | `explore(target=None, project_dir=".", database=None, catalog=None, quack_token=None, insecure=False, open_window=True, wait=True, env=None) -> None`<br>Opens the Terminal IDE against a live `duckdb.DuckDBPyConnection`, file/URI string, or `None` (resolves `duckpack.toml` / Workspace Mode). |
| **`apply`** | `apply(target=None, project_dir=".", database=None, catalog=None, auto_approve=True, force_drop=False, interactive=False, quack_token=None, insecure=False, open_window=True, wait=True, env=None) -> ApplyResult`<br>Computes diffs and applies declarative migrations. Set `DUCKPACK_ALLOW_LAKE_REWRITE=true` in the environment to permit DuckLake copy-and-swap rewrites. |
| **`diff`** | `diff(target=None, project_dir=".", database=None, catalog=None, quack_token=None, insecure=False, env=None) -> DiffSummary`<br>Detects schema drift without mutating state. |
| **`build`** | `build(project_dir=".") -> None`<br>Compiles and validates all SQL files against an isolated in-memory Shadow DB. |
| **`compile`** | `compile(project_dir, out) -> None`<br>Packages project declarations into an immutable `.duckpack` snapshot artifact. |
| **`init`** | `init(project_dir=".") -> None`<br>Scaffolds a new declarative DuckPack project directory. |
| **`get_target_config`** | `get_target_config(project_dir=".", env=None) -> dict`<br>Inspects resolved target database, catalogs, credentials, and `workspace_mode` from `duckpack.toml` and `.env`. |
| **`stop_server`** | `stop_server() -> bool`<br>Stops any active background notebook `SessionBridge`. |

### Return Objects & Exceptions
- **`ApplyResult`**: `.success: bool`, `.applied_count: int`, `.items: list[str]`
- **`DiffSummary`**: `.has_changes: bool`, `.change_count: int`, `.items: list[str]`
- **`SessionBridge`**: Ephemeral localhost Quack bridge class used for live in-memory Python connection inspection and two-way temporary table synchronization.
- **Exceptions**: `duckpack.Error` (base) → `duckpack.SchemaError`, `duckpack.ExecutionError`.

---

## Managing Python Bindings from the CLI

```bash
# Check active virtualenv ($VIRTUAL_ENV / ./.venv), installer (uv vs pip), and versions
duckpack python status

# Install or upgrade duckpack in the active Python environment
duckpack python install --upgrade
duckpack python install --uv -U
duckpack python install --pip -U
```
