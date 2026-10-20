"""
DuckPack Interaction Runner:
Orchestrates environment detection and execution strategies across
Direct TTY, Desktop Terminal Windows, Dev Container Notebook Cards, and Headless Consoles.
"""

import sys
from typing import Any, Callable, Optional


def is_interactive_tty() -> bool:
    """Returns True if running in an active interactive terminal session (TTY)."""
    return bool(sys.stdin and sys.stdin.isatty() and sys.stdout and sys.stdout.isatty())


class InteractionRunner:
    """
    Centralized execution dispatcher that evaluates runtime environment capabilities
    and selects the appropriate presentation modality for DuckPack interactive tools.
    """

    @classmethod
    def dispatch(
        cls,
        mode: str,
        target: Any,
        project_dir: str = ".",
        database: Optional[str] = None,
        force_drop: bool = False,
        quack_token: Optional[str] = None,
        insecure: bool = False,
        open_window: bool = True,
        wait: bool = True,
        env: Optional[str] = None,
        on_tty: Optional[Callable[[], Any]] = None,
        on_window_applied: Optional[Callable[[], Any]] = None,
        on_headless: Optional[Callable[[], Any]] = None,
    ) -> Any:
        """
        Executes an interactive command across the 4-tier environment hierarchy:
        1. Direct TTY (in-process Ratatui TUI)
        2. Desktop GUI Terminal Window (external pop-out like Ghostty, Foot, Terminal.app)
        3. Notebook Environment (rich HTML card with copyable terminal command block)
        4. Headless Console Fallback (plain text output or diagnostic error)
        """
        # 1. Direct interactive TTY shell
        if is_interactive_tty():
            if on_tty:
                return on_tty()

        # 2. Desktop Window Manager (Wayland/X11/macOS/Windows) with terminal emulator
        if open_window:
            from .terminal import launch_terminal_window

            launched = launch_terminal_window(
                target,
                project_dir=project_dir,
                database=database,
                mode=mode,
                force_drop=force_drop,
                quack_token=quack_token,
                insecure=insecure,
                wait=wait,
                env=env,
            )
            if launched:
                if on_window_applied:
                    return on_window_applied()
                return None

        # 3. Dev Containers, headless environments, or remote SSH without a GUI display server
        from .notebook import is_notebook_environment, render_notebook_card

        if is_notebook_environment():
            if render_notebook_card(
                target,
                project_dir=project_dir,
                database=database,
                mode=mode,
                force_drop=force_drop,
                quack_token=quack_token,
                insecure=insecure,
                env=env,
            ):
                if mode == "apply":
                    from ._duckpack import ApplyResult

                    return ApplyResult(success=True, applied_count=0, items=[])
                return None

        # 4. Plain console fallback (headless CI / non-notebook script)
        if on_headless:
            return on_headless()
        return None
