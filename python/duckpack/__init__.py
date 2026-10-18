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
)

def explore(
    target,
    project_dir=".",
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
    """
    project_dir = str(project_dir)

    # 1. If running in an active terminal shell, run directly in-process
    if sys.stdin and sys.stdin.isatty() and sys.stdout and sys.stdout.isatty():
        return _native_explore(
            target,
            project_dir=project_dir,
            quack_token=quack_token,
            insecure=insecure,
        )

    # 2. In non-TTY environment (e.g. Jupyter notebook in VS Code):
    if open_window:
        from .terminal import launch_terminal_window
        launched = launch_terminal_window(
            target,
            project_dir=project_dir,
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
        quack_token=quack_token,
        insecure=insecure,
    )


def apply(
    target,
    project_dir=".",
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
    - auto_approve: apply migrations automatically without user confirmation
    - force_drop: permit destructive drops (tables/columns/views)
    - interactive: request interactive review before applying
    - quack_token: auth token for remote Quack connections
    - insecure: disable SSL for local/plain HTTP Quack connections
    """
    project_dir = str(project_dir)

    # In non-TTY environments (such as Jupyter notebooks in VS Code):
    if not auto_approve or interactive:
        diff_summary = diff(
            target,
            project_dir=project_dir,
            quack_token=quack_token,
            insecure=insecure,
        )
        if not diff_summary.has_changes:
            return ApplyResult(success=True, applied_count=0, items=[])

        # If not in an interactive terminal (e.g. Jupyter notebook):
        if not (sys.stdin and sys.stdin.isatty() and sys.stdout and sys.stdout.isatty()):
            print(f"🦆 DuckPack: Found {diff_summary.change_count} change(s) to apply:")
            for item in diff_summary.items:
                print(f"  • {item}")
            try:
                ans = input("Apply changes? [y/N]: ")
                if ans.strip().lower() in ("y", "yes"):
                    return _native_apply(
                        target,
                        project_dir=project_dir,
                        auto_approve=True,
                        force_drop=force_drop,
                        interactive=False,
                        quack_token=quack_token,
                        insecure=insecure,
                    )
                else:
                    print("Migration cancelled by user.")
                    return ApplyResult(success=False, applied_count=0, items=[])
            except (EOFError, OSError):
                if not auto_approve:
                    raise ExecutionError(
                        "Cannot apply schema changes: auto_approve is False in a non-interactive environment. "
                        "Pass auto_approve=True to approve and apply migrations automatically."
                    )

    return _native_apply(
        target,
        project_dir=project_dir,
        auto_approve=auto_approve,
        force_drop=force_drop,
        interactive=interactive,
        quack_token=quack_token,
        insecure=insecure,
    )


def diff(target, project_dir=".", quack_token=None, insecure=False):
    """Compute differences between project declarative definitions and the target database."""
    return _native_diff(
        target,
        project_dir=str(project_dir),
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
]

__version__ = "0.2.0"
