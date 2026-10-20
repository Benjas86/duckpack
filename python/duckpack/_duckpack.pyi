from typing import Any, Dict, List, Optional, Union
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

class SessionBridge:
    def __init__(self, target: Any, token: Optional[str] = None, port: Optional[int] = None) -> None: ...
    def start(self) -> str: ...
    def stop(self) -> bool: ...
    @property
    def db_uri(self) -> str: ...
    @property
    def port(self) -> int: ...
    @property
    def token(self) -> str: ...
    @property
    def is_active(self) -> bool: ...
    @staticmethod
    def has_temp_objects(target: Any) -> bool: ...
    def __enter__(self) -> "SessionBridge": ...
    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> bool: ...

def stage_host_temp_tables(target: Any) -> tuple[List[str], List[tuple[str, str]]]: ...
def sync_back_host_temp_objects(target: Any) -> None: ...

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
    target: Optional[Union[duckdb.DuckDBPyConnection, str]] = None,
    project_dir: str = ".",
    database: Optional[str] = None,
    catalog: Optional[str] = None,
    quack_token: Optional[str] = None,
    insecure: bool = False,
    env: Optional[str] = None,
) -> DiffSummary:
    """Compute differences between project declarative definitions and the target database."""
    ...

def apply(
    target: Optional[Union[duckdb.DuckDBPyConnection, str]] = None,
    project_dir: str = ".",
    database: Optional[str] = None,
    catalog: Optional[str] = None,
    auto_approve: bool = True,
    force_drop: bool = False,
    interactive: bool = False,
    quack_token: Optional[str] = None,
    insecure: bool = False,
    env: Optional[str] = None,
) -> ApplyResult:
    """Apply declarative schema migrations to the target database."""
    ...

def explore(
    target: Optional[Union[duckdb.DuckDBPyConnection, str]] = None,
    project_dir: str = ".",
    database: Optional[str] = None,
    catalog: Optional[str] = None,
    quack_token: Optional[str] = None,
    insecure: bool = False,
    open_window: bool = True,
    wait: bool = True,
    env: Optional[str] = None,
) -> None:
    """Launch the interactive DuckPack IDE terminal UI against a target database or live connection.

    - In an interactive terminal shell (TTY), runs full-screen directly in-process.
    - In non-interactive environments (e.g. Jupyter Notebooks or VS Code Interactive):
      • When open_window=True (default), automatically launches a connected terminal window.
      • When open_window=False or in headless environments, prints a formatted schema overview.
    """
    ...

def get_target_config(project_dir: str = ".", env: Optional[str] = None) -> Dict[str, Any]:
    """Inspect resolved target configuration and credentials from duckpack.toml and .env."""
    ...
