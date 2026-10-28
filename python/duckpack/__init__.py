"""
DuckPack: Declarative schema management, interactive terminal IDE, and in-memory inspection for DuckDB.
"""

import os
import sys
from pathlib import Path
from ._duckpack import (
    Error,
    SchemaError,
    ExecutionError,
    ApplyResult,
    DiffSummary,
    explore as _native_explore,
    apply as _native_apply,
    diff as _native_diff,
    compile as _native_compile,
    build as _native_build,
    init as _native_init,
    get_target_config as _native_get_target_config,
    SessionBridge,
    __version__ as _native_version,
    __duckdb_version__,
)
from .runner import InteractionRunner


def _is_db_conn(obj):
    return obj is not None and hasattr(obj, "execute") and not isinstance(obj, (str, Path))


def _discover_project_dir(start="."):
    """Auto-discovers the nearest declarative DuckPack project directory."""
    try:
        start_path = Path(start).resolve()
    except Exception:
        return start

    def is_proj(p):
        return (
            (p / "duckpack.toml").exists()
            or (p / "queries").is_dir()
            or (p / "tables").is_dir()
            or (p / "schemas").is_dir()
        )

    if is_proj(start_path):
        return str(start_path)

    # 1. Candidate subdirectories
    for cand in ["cli", "duckpack", "duckdb_project", "duckdb", "project", "demo_project"]:
        sub = start_path / cand
        if sub.is_dir() and is_proj(sub):
            return str(sub)

    # 2. Check parent directories up to filesystem root
    curr = start_path.parent
    while curr and curr != curr.parent:
        if is_proj(curr):
            return str(curr)
        curr = curr.parent

    # 3. Check sibling directories under parent (e.g. from 0.2.0/python to 0.2.0/cli)
    parent = start_path.parent
    if parent and parent != start_path:
        for cand in ["cli", "duckpack", "duckdb_project", "duckdb", "project", "demo_project"]:
            sib = parent / cand
            if sib.is_dir() and is_proj(sib):
                return str(sib)

    return start


def _normalize_session_args(target, project_dir, database, catalog):
    # Case A: user passed connection as database=con or catalog=con
    if target is None:
        if _is_db_conn(database):
            target = database
            database = None
        elif _is_db_conn(catalog):
            target = catalog
            catalog = None

    # Case B: user passed (project_dir, con) as positional args: explore("../cli", con)
    if _is_db_conn(project_dir) and (isinstance(target, (str, Path)) or target is None):
        target, project_dir = project_dir, target or "."

    project_dir = str(project_dir) if project_dir is not None else "."
    active_cat = database or catalog

    # Discover project_dir if default "." has no project markers
    if project_dir == ".":
        project_dir = _discover_project_dir(project_dir)

    return target, project_dir, active_cat


def _auto_attach_toml_target_if_needed(target, project_dir=".", env=None):
    """
    If target is an active DuckDB connection and duckpack.toml defines an external database,
    automatically ATTACH it into the session if not already present.
    """
    if not hasattr(target, "execute"):
        return

    try:
        cfg = _native_get_target_config(project_dir=project_dir, env=env)
    except Exception:
        return

    db_target = cfg.get("db")
    if not db_target:
        return

    catalogs = cfg.get("catalogs") or []
    catalog_alias = catalogs[0] if catalogs else None

    # If no explicit warehouse catalog is specified and db_target is just the local default database file,
    # do NOT auto-attach it to an explicit in-memory connection session.
    is_remote = any(db_target.startswith(p) for p in ("quack:", "quack://", "md:", "motherduck:"))
    if not catalog_alias and not is_remote and env is None:
        return

    if not catalog_alias:
        if is_remote:
            catalog_alias = "remote_db"
        else:
            catalog_alias = Path(db_target).stem or "remote_db"

    # Check if catalog_alias is already attached in this connection
    try:
        existing_dbs = [r[0] for r in target.execute("SELECT database_name FROM duckdb_databases();").fetchall()]
        if catalog_alias in existing_dbs:
            return
    except Exception:
        pass

    token = cfg.get("token")
    insecure = cfg.get("insecure", False)

    try:
        if db_target.startswith("quack:") or db_target.startswith("quack://"):
            target.execute("INSTALL quack; LOAD quack;")
            if token:
                safe_token = str(token).replace("'", "''")
                target.execute(f"CREATE SECRET (TYPE QUACK, TOKEN '{safe_token}');")
            disable_ssl = "true" if insecure else "false"
            safe_db = db_target.replace("'", "''")
            target.execute(f"ATTACH '{safe_db}' AS \"{catalog_alias}\" (DISABLE_SSL {disable_ssl});")
        elif db_target.startswith("md:") or db_target.startswith("motherduck:"):
            safe_db = db_target.replace("'", "''")
            target.execute(f"ATTACH '{safe_db}' AS \"{catalog_alias}\";")
        else:
            safe_db = db_target.replace("'", "''")
            target.execute(f"ATTACH '{safe_db}' AS \"{catalog_alias}\";")

        print(f"[DuckPack] Attached '{catalog_alias}' ({db_target}) to Python connection.")
    except Exception as e:
        print(f"[DuckPack] Warning: Could not auto-attach '{catalog_alias}' ({db_target}): {e}. Proceeding with in-memory session.")


def diff(
    target=None,
    project_dir=".",
    database=None,
    catalog=None,
    quack_token=None,
    insecure=False,
    env=None,
):
    """
    Compute differences between project declarative definitions and the target database.

    - target: duckdb connection (con), string path/URI, or None (auto-resolves from duckpack.toml)
    - project_dir: path to directory with declarative definitions
    - database: target database catalog name when multiple databases are attached (alias: catalog)
    - quack_token: auth token for remote Quack connections (defaults to .env QUACK_TOKEN)
    - insecure: disable SSL for local/plain HTTP Quack connections
    - env: optional environment profile name (e.g. 'staging', 'prod') to load from duckpack.toml and .env.<env>
    """
    target, project_dir, active_cat = _normalize_session_args(
        target, project_dir, database, catalog
    )

    if target is None:
        cfg = _native_get_target_config(project_dir=project_dir, env=env)
        target = cfg.get("db") or "local.duckdb"
        if not any(str(target).startswith(p) for p in ("quack:", "quack://", "md:", "motherduck:")) and not os.path.isabs(str(target)):
            cand = os.path.join(project_dir, str(target))
            if os.path.exists(cand):
                target = cand
        if not active_cat:
            catalogs = cfg.get("catalogs") or []
            if catalogs:
                active_cat = catalogs[0]
        if quack_token is None:
            quack_token = cfg.get("token")
        if not insecure:
            insecure = cfg.get("insecure", False)

    return _native_diff(
        target,
        project_dir=project_dir,
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
        env=env,
    )


def apply(
    target=None,
    project_dir=".",
    database=None,
    catalog=None,
    auto_approve=True,
    force_drop=False,
    interactive=False,
    quack_token=None,
    insecure=False,
    open_window=True,
    wait=True,
    env=None,
):
    """
    Apply declarative schema migrations to the target database.

    - target: duckdb connection (con), string path/URI, or None (auto-resolves from duckpack.toml)
    - project_dir: path to directory with declarative definitions (str or Path)
    - database: target database catalog name when multiple databases are attached (alias: catalog)
    - auto_approve: apply migrations automatically without user confirmation
    - force_drop: permit destructive drops (tables/columns/views)
    - interactive: launch interactive Ratatui Diff & Apply TUI screen
    - quack_token: auth token for remote Quack connections
    - insecure: disable SSL for local/plain HTTP Quack connections
    - env: optional environment profile name (e.g. 'staging', 'prod') to load from duckpack.toml and .env.<env>
    """
    target, project_dir, active_cat = _normalize_session_args(
        target, project_dir, database, catalog
    )

    if target is None:
        cfg = _native_get_target_config(project_dir=project_dir, env=env)
        target = cfg.get("db") or "local.duckdb"
        if not any(str(target).startswith(p) for p in ("quack:", "quack://", "md:", "motherduck:")) and not os.path.isabs(str(target)):
            cand = os.path.join(project_dir, str(target))
            if os.path.exists(cand):
                target = cand
        if not active_cat:
            catalogs = cfg.get("catalogs") or []
            if catalogs:
                active_cat = catalogs[0]
        if quack_token is None:
            quack_token = cfg.get("token")
        if not insecure:
            insecure = cfg.get("insecure", False)

    is_tty = bool(sys.stdin and sys.stdin.isatty() and sys.stdout and sys.stdout.isatty())

    # Fast path: automated execution without review
    if auto_approve and not interactive:
        return _native_apply(
            target,
            project_dir=project_dir,
            database=active_cat,
            auto_approve=True,
            force_drop=force_drop,
            interactive=False,
            quack_token=quack_token,
            insecure=insecure,
            env=env,
        )

    # Interactive review requested (either interactive=True or auto_approve=False)
    diff_before = diff(
        target,
        project_dir=project_dir,
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
        env=env,
    )
    if not diff_before.has_changes:
        cat_suffix = f" for catalog '{active_cat}'" if active_cat else ""
        print(f"🦆 DuckPack: Database is already up to date (0 pending changes){cat_suffix}.")
        return ApplyResult(success=True, applied_count=0, items=[])

    def run_tty():
        return _native_apply(
            target,
            project_dir=project_dir,
            database=active_cat,
            auto_approve=False,
            force_drop=force_drop,
            interactive=True,
            quack_token=quack_token,
            insecure=insecure,
            env=env,
        )

    def on_window():
        diff_after = diff(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
            env=env,
        )
        applied = [item for item in diff_before.items if item not in diff_after.items]
        if diff_after.change_count < diff_before.change_count or len(applied) > 0:
            print(f"🦆 DuckPack: Applied {len(applied)} change(s) successfully.")
            return ApplyResult(success=True, applied_count=len(applied), items=applied)
        else:
            print("🦆 DuckPack: Migration cancelled by user (0 changes applied).")
            return ApplyResult(success=False, applied_count=0, items=[])

    def on_headless():
        print(f"🦆 DuckPack: Found {diff_before.change_count} pending change(s) to apply:")
        for item in diff_before.items:
            print(f"  • {item}")
        target_str = target if isinstance(target, str) else "local.duckdb"
        print("\nℹ️  Dev Container / Headless Environment Detected.")
        print("To review and apply these changes interactively, run in any terminal panel:")
        print(f"  $ duckpack apply -p {project_dir} -d {target_str} --interactive\n")
        raise ExecutionError(
            "Cannot run interactive apply in a headless environment without an interactive terminal. "
            "Run the command above in a terminal, or pass auto_approve=True to apply automatically."
        )

    return InteractionRunner.dispatch(
        mode="apply",
        target=target,
        project_dir=project_dir,
        database=active_cat,
        force_drop=force_drop,
        quack_token=quack_token,
        insecure=insecure,
        open_window=open_window,
        wait=wait,
        env=env,
        on_tty=run_tty,
        on_window_applied=on_window,
        on_headless=on_headless,
    )


def explore(
    target=None,
    project_dir=".",
    database=None,
    catalog=None,
    quack_token=None,
    insecure=False,
    open_window=True,
    wait=True,
    env=None,
):
    """
    Launch the interactive DuckPack terminal IDE against a database.

    - target: duckdb connection (con), string path/URI, or None (auto-resolves from duckpack.toml)
    - If running inside an interactive terminal shell (TTY), runs full-screen
      directly in the current terminal.
    - If running inside a non-interactive environment (such as a Jupyter Notebook
      in VS Code):
      • When `open_window=True` (default), automatically pops open a graphical
        terminal window (Ghostty, Foot, Terminal.app, Windows Terminal) connected
        to the database (including live in-memory DuckDB connections).
      • In headless environments or Dev Containers without a desktop window manager,
        renders an informative card with a 1-click copy block to launch the full-screen
        interactive TUI in any VS Code terminal panel.
    - database: Target database catalog name when multiple databases are attached (alias: catalog).
    - env: optional environment profile name (e.g. 'staging', 'prod') to load from duckpack.toml and .env.<env>
    """
    target, project_dir, active_cat = _normalize_session_args(
        target, project_dir, database, catalog
    )

    if target is None:
        cfg = _native_get_target_config(project_dir=project_dir, env=env)
        target = cfg.get("db") or "local.duckdb"
        if not any(str(target).startswith(p) for p in ("quack:", "quack://", "md:", "motherduck:")) and not os.path.isabs(str(target)):
            cand = os.path.join(project_dir, str(target))
            if os.path.exists(cand):
                target = cand
        if not active_cat:
            catalogs = cfg.get("catalogs") or []
            if catalogs:
                active_cat = catalogs[0]
        if quack_token is None:
            quack_token = cfg.get("token")
        if not insecure:
            insecure = cfg.get("insecure", False)
    else:
        # If an existing Python connection was passed, auto-attach TOML target if configured
        _auto_attach_toml_target_if_needed(target, project_dir=project_dir, env=env)

    return InteractionRunner.dispatch(
        mode="explore",
        target=target,
        project_dir=project_dir,
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
        open_window=open_window,
        wait=wait,
        env=env,
        on_tty=lambda: _native_explore(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
            env=env,
        ),
        on_headless=lambda: _native_explore(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
            env=env,
        ),
    )


def compile(project_dir, out):
    """Compile project SQL definitions into an immutable .duckpack snapshot file."""
    return _native_compile(str(project_dir), str(out))


def build(project_dir="."):
    """Validate project SQL syntax against an in-memory Shadow DB."""
    if project_dir == ".":
        project_dir = _discover_project_dir(project_dir)
    return _native_build(str(project_dir))


def init(project_dir="."):
    """Scaffold a new declarative database project directory."""
    return _native_init(str(project_dir))


def get_target_config(project_dir=".", env=None):
    """Inspect resolved target configuration and credentials from duckpack.toml and .env."""
    if project_dir == ".":
        project_dir = _discover_project_dir(project_dir)
    return _native_get_target_config(str(project_dir), env)


from .notebook import stop_server


__all__ = [
    "Error",
    "SchemaError",
    "ExecutionError",
    "ApplyResult",
    "DiffSummary",
    "explore",
    "apply",
    "diff",
    "compile",
    "build",
    "init",
    "get_target_config",
    "stop_server",
    "SessionBridge",
    "InteractionRunner",
    "__version__",
    "__duckdb_version__",
]

__version__ = _native_version
