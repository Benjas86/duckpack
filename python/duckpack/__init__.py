"""
DuckPack: Declarative schema management, interactive terminal IDE, and in-memory inspection for DuckDB.
"""

from ._duckpack import (
    Error,
    SchemaError,
    ExecutionError,
    ApplyResult,
    DiffSummary,
    explore,
    apply,
    diff,
    compile,
    build,
    init,
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
