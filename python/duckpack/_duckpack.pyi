from typing import Any, List, Optional, Union
import duckdb

class Error(Exception): ...
class SchemaError(Error): ...
class ExecutionError(Error): ...

class ApplyResult:
    success: bool
    applied_count: int
    items: List[str]

class DiffSummary:
    has_changes: bool
    change_count: int
    items: List[str]

def init(project_dir: str = ".") -> None:
    """Initialize a new declarative DuckDB project scaffold."""
    ...

def build(project_dir: str = ".") -> None:
    """Build and validate project SQL syntax against an in-memory Shadow DB."""
    ...

def compile(project_dir: str, out: str) -> None:
    """Compile a declarative DuckDB project into an immutable `.duckpack` snapshot artifact."""
    ...

def diff(
    target: Union[duckdb.DuckDBPyConnection, str],
    project_dir: str = ".",
    quack_token: Optional[str] = None,
    insecure: bool = False,
) -> DiffSummary:
    """Compute differences between project declarative definitions and the target database."""
    ...

def apply(
    target: Union[duckdb.DuckDBPyConnection, str],
    project_dir: str = ".",
    auto_approve: bool = True,
    force_drop: bool = False,
    interactive: bool = False,
    quack_token: Optional[str] = None,
    insecure: bool = False,
) -> ApplyResult:
    """Apply declarative schema migrations to the target database."""
    ...

def explore(
    target: Union[duckdb.DuckDBPyConnection, str],
    project_dir: str = ".",
    quack_token: Optional[str] = None,
    insecure: bool = False,
    open_window: bool = True,
    wait: bool = True,
) -> None:
    """Launch the interactive DuckPack IDE terminal UI against a target database or live connection.

    - In an interactive terminal shell (TTY), runs full-screen directly in-process.
    - In non-interactive environments (e.g. Jupyter Notebooks or VS Code Interactive):
      • When open_window=True (default), automatically launches a connected terminal window.
      • When open_window=False or in headless environments, prints a formatted schema overview.
    """
    ...
