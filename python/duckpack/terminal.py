"""
DuckPack Terminal Launcher:
Automatically launches an interactive terminal emulator window running the
DuckPack TUI IDE when called from non-interactive environments like Jupyter
Notebooks or VS Code Interactive.
"""

import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile


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
        # macOS: Pipe AppleScript to osascript over stdin (never exposes secrets in argv/ps)
        return ("Terminal.app", ["osascript", "-"])
    elif sys.platform == "win32":
        if shutil.which("wt.exe"):
            return ("Windows Terminal", ["wt.exe"])
        # H2: Direct CREATE_NEW_CONSOLE spawn avoids cmd.exe /c start metacharacter injection
        return ("Command Prompt", [])
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
    if isinstance(target, (str, Path)):
        db_arg = str(target)
        token_arg = quack_token
        insecure_arg = insecure
    else:
        # Live Python connection object (in-memory or file-backed):
        # Spawning an external terminal process to open the same database file on disk
        # will collide with Python's active write lock ("Conflicting lock is held").
        # Always use SessionBridge to connect via loopback Quack without file locks!
        bridge = SessionBridge(target)
        try:
            db_arg = bridge.start()
        except Exception as e:
            print(f"Warning: Could not start local quack bridge for Python connection: {e}")
            return False

        token_arg = bridge.token
        insecure_arg = True

        def cleanup():
            try:
                bridge.stop()
            except Exception:
                pass

        cleanup_fn = cleanup

    # 2. Build the command to run inside the terminal
    eff_proj = project_dir
    if eff_proj == ".":
        from . import _discover_project_dir

        eff_proj = _discover_project_dir(eff_proj)

    if mode == "apply":
        cli_cmd = [sys.executable, "-m", "duckpack", "apply", "-p", eff_proj, "-d", db_arg, "--interactive"]
        if force_drop:
            cli_cmd.append("--force-drop")
    else:
        cli_cmd = [sys.executable, "-m", "duckpack", "explore", "-p", eff_proj, "-d", db_arg]

    if active_cat:
        cli_cmd.extend(["--database", active_cat])
    if insecure_arg:
        cli_cmd.append("--insecure")
    if env:
        cli_cmd.extend(["--env", env])

    child_env = os.environ.copy()
    if token_arg:
        child_env["QUACK_TOKEN"] = str(token_arg)

    # 3. Launch the terminal emulator
    try:
        if sys.platform == "darwin":
            shell_parts = [shlex.quote(str(arg)) for arg in cli_cmd]
            cmd_prefix = ""
            if token_arg:
                clean_tok = str(token_arg).replace("\r", "").replace("\n", "").replace("\0", "")
                fd, tok_path = tempfile.mkstemp(prefix="duckpack_tok_", suffix=".sh")
                try:
                    os.chmod(tok_path, 0o600)
                    with os.fdopen(fd, "w") as tf:
                        tf.write(f"export QUACK_TOKEN={shlex.quote(clean_tok)}\n")
                except Exception:
                    try:
                        os.unlink(tok_path)
                    except OSError:
                        pass
                    raise
                quoted_tok_path = shlex.quote(tok_path)
                cmd_prefix = f". {quoted_tok_path} && rm -f {quoted_tok_path} && "
            cmd_str = cmd_prefix + " ".join(shell_parts)
            # H2: Strip CR/LF and pipe AppleScript over stdin so neither command nor token enters argv
            apple_escaped = (
                cmd_str.replace("\r", "")
                .replace("\n", " ")
                .replace("\\", "\\\\")
                .replace('"', '\\"')
            )
            script = f'tell application "Terminal" to do script "{apple_escaped}"\n'
            proc = subprocess.Popen(
                ["osascript", "-"],
                stdin=subprocess.PIPE,
                env=child_env,
            )
            if proc.stdin is not None:
                proc.stdin.write(script.encode("utf-8"))
                proc.stdin.close()
        elif sys.platform == "win32" and "wt.exe" in term_prefix:
            proc = subprocess.Popen(term_prefix + cli_cmd, env=child_env)
        elif sys.platform == "win32":
            # H2: Launch directly in a new console window without cmd.exe shell parsing
            creationflags = getattr(subprocess, "CREATE_NEW_CONSOLE", 0)
            proc = subprocess.Popen(cli_cmd, env=child_env, creationflags=creationflags)
        else:
            # Linux (Ghostty, Foot, Kitty, Alacritty, etc.)
            full_cmd = term_prefix + cli_cmd
            proc = subprocess.Popen(full_cmd, env=child_env)

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
