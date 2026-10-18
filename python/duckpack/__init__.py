"""
DuckPack: Declarative schema management, interactive terminal IDE, and in-memory inspection for DuckDB.
"""

import sys
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
    __duckdb_version__,
)

def explore(
    target,
    project_dir=".",
    database=None,
    catalog=None,
    quack_token=None,
    insecure=False,
    open_window=True,
    wait=True,
):
    """
    Launch the interactive DuckPack terminal IDE against a database.

    - If running inside an interactive terminal shell (TTY), runs full-screen
      directly in the current terminal.
    - If running inside a non-interactive environment (such as a Jupyter Notebook
      in VS Code):
      • When `open_window=True` (default), automatically pops open a graphical
        terminal window (Ghostty, Foot, Terminal.app, Windows Terminal) connected
        to the database (including live in-memory DuckDB connections).
      • When `open_window=False` or in headless environments (no GUI display),
        prints a clean, formatted schema overview directly into the notebook cell.
    - database: Target database catalog name when multiple databases are attached (alias: catalog).
    """
    project_dir = str(project_dir)
    active_cat = database or catalog

    # 1. If running in an active terminal shell, run directly in-process
    if sys.stdin and sys.stdin.isatty() and sys.stdout and sys.stdout.isatty():
        return _native_explore(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
        )

    # 2. In non-TTY environment (e.g. Jupyter notebook in VS Code):
    if open_window:
        from .terminal import launch_terminal_window
        launched = launch_terminal_window(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
            wait=wait,
        )
        if launched:
            return None

    # 3. Fallback to in-cell formatted overview
    return _native_explore(
        target,
        project_dir=project_dir,
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
    )


def apply(
    target,
    project_dir=".",
    database=None,
    catalog=None,
    auto_approve=True,
    force_drop=False,
    interactive=False,
    quack_token=None,
    insecure=False,
):
    """
    Apply declarative schema migrations to the target database.

    - target: duckdb connection (con) or string path/URI
    - project_dir: path to directory with declarative definitions (str or Path)
    - database: target database catalog name when multiple databases are attached (alias: catalog)
    - auto_approve: apply migrations automatically without user confirmation
    - force_drop: permit destructive drops (tables/columns/views)
    - interactive: launch interactive Ratatui Diff & Apply TUI screen
    - quack_token: auth token for remote Quack connections
    - insecure: disable SSL for local/plain HTTP Quack connections
    """
    project_dir = str(project_dir)
    active_cat = database or catalog

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
        )

    # Interactive review requested (either interactive=True or auto_approve=False)
    diff_before = diff(
        target,
        project_dir=project_dir,
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
    )
    if not diff_before.has_changes:
        cat_suffix = f" for catalog '{active_cat}'" if active_cat else ""
        print(f"🦆 DuckPack: Database is already up to date (0 pending changes){cat_suffix}.")
        return ApplyResult(success=True, applied_count=0, items=[])

    # 1. If running inside an interactive terminal shell (TTY), run in-process TUI:
    if is_tty:
        return _native_apply(
            target,
            project_dir=project_dir,
            database=active_cat,
            auto_approve=False,
            force_drop=force_drop,
            interactive=True,
            quack_token=quack_token,
            insecure=insecure,
        )

    # 2. In non-TTY environments (e.g. Jupyter Notebook in VS Code):
    # Automatically pop open external connected terminal window with the full Diff & Apply TUI
    from .terminal import launch_terminal_window
    launched = launch_terminal_window(
        target,
        project_dir=project_dir,
        database=active_cat,
        mode="apply",
        force_drop=force_drop,
        quack_token=quack_token,
        insecure=insecure,
        wait=True,
    )
    if launched:
        diff_after = diff(
            target,
            project_dir=project_dir,
            database=active_cat,
            quack_token=quack_token,
            insecure=insecure,
        )
        applied = [item for item in diff_before.items if item not in diff_after.items]
        if diff_after.change_count < diff_before.change_count or len(applied) > 0:
            print(f"🦆 DuckPack: Applied {len(applied)} change(s) successfully.")
            return ApplyResult(success=True, applied_count=len(applied), items=applied)
        else:
            print("🦆 DuckPack: Migration cancelled by user (0 changes applied).")
            return ApplyResult(success=False, applied_count=0, items=[])

    # 3. Headless fallback (no display / CI / Docker container without GUI):
    print(f"🦆 DuckPack: Found {diff_before.change_count} pending change(s) to apply:")
    for item in diff_before.items:
        print(f"  • {item}")
    raise ExecutionError(
        "Cannot run interactive apply in a headless environment without a display. "
        "Pass auto_approve=True to apply automatically."
    )


def diff(target, project_dir=".", database=None, catalog=None, quack_token=None, insecure=False):
    """Compute differences between project declarative definitions and the target database."""
    active_cat = database or catalog
    return _native_diff(
        target,
        project_dir=str(project_dir),
        database=active_cat,
        quack_token=quack_token,
        insecure=insecure,
    )


def compile(project_dir, out):
    """Compile project SQL definitions into an immutable .duckpack snapshot file."""
    return _native_compile(str(project_dir), str(out))


def build(project_dir="."):
    """Validate project SQL syntax against an in-memory Shadow DB."""
    return _native_build(str(project_dir))


def init(project_dir="."):
    """Scaffold a new declarative database project directory."""
    return _native_init(str(project_dir))


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
    "__version__",
    "__duckdb_version__",
]

__version__ = "0.2.0"
