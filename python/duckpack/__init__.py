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
    apply,
    diff,
    compile,
    build,
    init,
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
