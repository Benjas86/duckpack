"""
DuckPack Terminal Launcher:
Automatically launches an interactive terminal emulator window running the
DuckPack TUI IDE when called from non-interactive environments like Jupyter
Notebooks or VS Code Interactive.
"""

import os
import shutil
import socket
import subprocess
import sys


from ._duckpack import SessionBridge, stage_host_temp_tables, sync_back_host_temp_objects


def stage_connection_temp_objects(target):
    """Backward-compatible helper: delegates to deep Rust staging module."""
    try:
        stage_host_temp_tables(target)
    except Exception:
        pass


def sync_back_connection_temp_objects(target):
    """Backward-compatible helper: delegates to deep Rust staging module."""
    try:
        sync_back_host_temp_objects(target)
    except Exception:
        pass


def find_terminal_emulator():
    """
    Finds a supported graphical terminal emulator on the system.
    Returns (terminal_name, command_prefix_list) or None.
    """
    if sys.platform == "darwin":
        # macOS: Use AppleScript to tell Terminal.app to run the command
        return ("Terminal.app", ["osascript", "-e"])
    elif sys.platform == "win32":
        if shutil.which("wt.exe"):
            return ("Windows Terminal", ["wt.exe"])
        return ("Command Prompt", ["cmd.exe", "/c", "start"])
    else:
        # Linux / BSD (Wayland & X11)
        # Check if a display server is present
        if not (os.environ.get("WAYLAND_DISPLAY") or os.environ.get("DISPLAY")):
            return None

        # Prioritize modern and common terminals
        candidates = [
            ("ghostty", ["ghostty", "-e"]),
            ("foot", ["foot"]),
            ("kitty", ["kitty"]),
            ("alacritty", ["alacritty", "-e"]),
            ("wezterm", ["wezterm", "start", "--"]),
            ("gnome-terminal", ["gnome-terminal", "--"]),
            ("konsole", ["konsole", "-e"]),
            ("xfce4-terminal", ["xfce4-terminal", "-e"]),
            ("xterm", ["xterm", "-e"]),
            ("x-terminal-emulator", ["x-terminal-emulator", "-e"]),
        ]
        for name, prefix in candidates:
            if shutil.which(name):
                return (name, prefix)
        return None


def launch_terminal_window(
    target,
    project_dir=".",
    database=None,
    catalog=None,
    mode="explore",
    force_drop=False,
    quack_token=None,
    insecure=False,
    wait=True,
    env=None,
) -> bool:
    """
    Spawns an external terminal window running DuckPack connected to the target database.
    - mode="explore": Launches the full-screen Ratatui Terminal IDE.
    - mode="apply": Launches the interactive Ratatui Diff & Apply migration screen.
    Returns True if successfully launched, False otherwise.
    """
    if "pytest" in sys.modules or os.environ.get("PYTEST_CURRENT_TEST"):
        return False

    term_info = find_terminal_emulator()
    if not term_info:
        return False

    term_name, term_prefix = term_info
    active_cat = database or catalog

    # 1. Resolve database target and quack bridge if in-memory
    cleanup_fn = None
    if isinstance(target, str):
        db_arg = target
        token_arg = quack_token
        insecure_arg = insecure
    else:
        # Python connection object
        db_path = None
        try:
            db_info = target.execute("PRAGMA database_list;").fetchall()
            if db_info and len(db_info[0]) >= 3:
                db_path = db_info[0][2]
        except Exception:
            db_path = None

        has_temp = False
        try:
            has_temp = SessionBridge.has_temp_objects(target)
        except Exception:
            pass

        if db_path and db_path != ":memory:" and os.path.exists(db_path) and not has_temp:
            db_arg = db_path
            token_arg = None
            insecure_arg = False
        else:
            # Live in-memory DuckDB connection or connection with temporary tables! Spin up local Quack bridge via SessionBridge:
            bridge = SessionBridge(target)
            try:
                db_arg = bridge.start()
            except Exception as e:
                print(f"Warning: Could not start local quack bridge for in-memory DB: {e}")
                return False

            token_arg = bridge.token
            insecure_arg = False

            def cleanup():
                try:
                    bridge.stop()
                except Exception:
                    pass

            cleanup_fn = cleanup

    # 2. Build the command to run inside the terminal
    eff_proj = project_dir
    if eff_proj == ".":
        for cand in ["duckpack", "duckdb_project", "duckdb", "demo_project"]:
            if os.path.isdir(cand) and (
                os.path.isdir(os.path.join(cand, "queries"))
                or os.path.exists(os.path.join(cand, "duckpack.toml"))
            ):
                eff_proj = cand
                break

    if mode == "apply":
        cli_cmd = [sys.executable, "-m", "duckpack", "apply", "-p", eff_proj, "-d", db_arg, "--interactive"]
        if force_drop:
            cli_cmd.append("--force-drop")
    else:
        cli_cmd = [sys.executable, "-m", "duckpack", "explore", "-p", eff_proj, "-d", db_arg]

    if active_cat:
        cli_cmd.extend(["--database", active_cat])
    if token_arg:
        cli_cmd.extend(["--quack-token", token_arg])
    if insecure_arg:
        cli_cmd.append("--insecure")
    if env:
        cli_cmd.extend(["--env", env])

    # 3. Launch the terminal emulator
    try:
        if sys.platform == "darwin":
            cmd_str = " ".join(f'"{arg}"' for arg in cli_cmd)
            script = f'tell application "Terminal" to do script "{cmd_str}"'
            proc = subprocess.Popen(["osascript", "-e", script])
        elif sys.platform == "win32" and "wt.exe" in term_prefix:
            proc = subprocess.Popen(term_prefix + cli_cmd)
        else:
            # Linux (Ghostty, Foot, Kitty, Alacritty, etc.)
            full_cmd = term_prefix + cli_cmd
            proc = subprocess.Popen(full_cmd)

        session_type = "Diff & Apply Screen" if mode == "apply" else "Terminal IDE"
        print(f"🦆 Launched DuckPack {session_type} in new window ({term_name})...")

        if wait:
            instructions = (
                "[Review diff and press Enter to apply, or q to cancel. Close the window to resume notebook...]"
                if mode == "apply"
                else "[Running interactively. Close the terminal window to return to notebook cell...]"
            )
            print(instructions)
            try:
                proc.wait()
            finally:
                if cleanup_fn:
                    cleanup_fn()
            print(f"✓ DuckPack {session_type.lower()} session closed.")
        return True
    except Exception as e:
        print(f"Warning: Failed to launch terminal window: {e}")
        if cleanup_fn:
            cleanup_fn()
        return False
