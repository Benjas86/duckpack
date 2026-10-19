"""
DuckPack Notebook Integration:
Provides rich, interactive rendering and terminal instructions for
Jupyter Notebooks, VS Code Interactive Window, and Dev Containers.
"""

import os
import socket
import sys
from pathlib import Path

# Track active background bridge for in-memory notebook sessions
_ACTIVE_NOTEBOOK_BRIDGE = None


def is_notebook_environment() -> bool:
    """Detects whether the code is running inside a Jupyter Notebook or interactive shell."""
    try:
        from IPython import get_ipython

        ip = get_ipython()
        if ip is not None:
            cls_name = ip.__class__.__name__
            if "ZMQ" in cls_name or "Notebook" in cls_name or hasattr(ip, "kernel"):
                return True
    except Exception:
        pass
    return False


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def stop_server() -> bool:
    """
    Stops any active DuckPack Quack bridge server created by explore() in the notebook.
    Returns True if an active server was stopped, False otherwise.
    """
    global _ACTIVE_NOTEBOOK_BRIDGE
    if _ACTIVE_NOTEBOOK_BRIDGE is not None:
        target = _ACTIVE_NOTEBOOK_BRIDGE.get("target")
        db_uri = _ACTIVE_NOTEBOOK_BRIDGE.get("db_uri")
        if target is not None and db_uri:
            try:
                from .terminal import sync_back_connection_temp_objects

                sync_back_connection_temp_objects(target)
            except Exception:
                pass
            try:
                target.execute(f"CALL quack_stop('{db_uri}');")
            except Exception:
                pass
        _ACTIVE_NOTEBOOK_BRIDGE = None
        return True
    return False


def introspect_schema_summary(target) -> list:
    """Extracts a table list with row count and column count for notebook preview."""
    tables = []
    if not hasattr(target, "execute"):
        return tables

    try:
        raw_tables = target.execute(
            """
            SELECT schema_name, table_name,
                   (SELECT count(*) FROM duckdb_columns() c
                    WHERE c.table_name = t.table_name AND c.schema_name = t.schema_name) as col_count
            FROM duckdb_tables() t
            WHERE NOT internal
            ORDER BY schema_name, table_name;
            """
        ).fetchall()
        for schema, name, col_cnt in raw_tables:
            safe_schema = schema.replace('"', '""')
            safe_name = name.replace('"', '""')
            try:
                row_cnt = target.execute(f'SELECT count(*) FROM "{safe_schema}"."{safe_name}";').fetchone()[0]
            except Exception:
                row_cnt = "?"
            tables.append({
                "schema": schema,
                "name": name,
                "columns": col_cnt,
                "rows": row_cnt,
            })
    except Exception:
        pass
    return tables


def render_notebook_card(
    target,
    project_dir=".",
    database=None,
    catalog=None,
    mode="explore",
    force_drop=False,
    quack_token=None,
    insecure=False,
    env=None,
) -> bool:
    """
    Renders an informative, high-clarity card in Jupyter / VS Code notebooks with a
    copyable code block to launch the full DuckPack TUI inside any VS Code terminal.
    """
    global _ACTIVE_NOTEBOOK_BRIDGE

    if not is_notebook_environment():
        return False

    try:
        from IPython.display import HTML, display
    except ImportError:
        return False

    active_cat = database or catalog
    db_arg = None
    token_arg = quack_token
    is_in_memory_bridge = False

    if isinstance(target, str):
        db_arg = target
    elif hasattr(target, "execute"):
        # Check if connected to a persistent database file
        db_path = None
        try:
            db_info = target.execute("PRAGMA database_list;").fetchall()
            if db_info and len(db_info[0]) >= 3:
                db_path = db_info[0][2]
        except Exception:
            db_path = None

        has_temp = False
        try:
            cnt = target.execute("SELECT count(*) FROM duckdb_tables() WHERE temporary = true AND internal = false;").fetchone()
            if cnt and cnt[0] > 0:
                has_temp = True
        except Exception:
            pass

        if db_path and db_path != ":memory:" and os.path.exists(db_path) and not has_temp:
            db_arg = db_path
        else:
            # In-memory session or temporary tables present -> start Quack bridge
            stop_server()  # Clean up any existing bridge first
            from .terminal import stage_connection_temp_objects

            stage_connection_temp_objects(target)
            port = find_free_port()
            db_uri = f"quack:127.0.0.1:{port}"
            token = "duckpack_py"
            try:
                target.execute("INSTALL quack; LOAD quack;")
                target.execute(f"CALL quack_serve('{db_uri}', token := '{token}');")
                _ACTIVE_NOTEBOOK_BRIDGE = {"port": port, "target": target, "db_uri": db_uri}
                db_arg = db_uri
                token_arg = token
                is_in_memory_bridge = True
            except Exception as e:
                print(f"[DuckPack] Warning: Could not start local Quack bridge: {e}")
                return False
    else:
        db_arg = "local.duckdb"

    # Build the terminal command
    cmd_parts = ["duckpack", mode]
    if project_dir and project_dir != ".":
        cmd_parts.extend(["-p", project_dir])
    if db_arg:
        cmd_parts.extend(["-d", db_arg])
    if active_cat:
        cmd_parts.extend(["--database", active_cat])
    if token_arg:
        cmd_parts.extend(["--quack-token", token_arg])
    if insecure:
        cmd_parts.append("--insecure")
    if env:
        cmd_parts.extend(["-e", env])
    if mode == "apply":
        cmd_parts.append("--interactive")
        if force_drop:
            cmd_parts.append("--force-drop")

    terminal_cmd = " ".join(cmd_parts)
    safe_cmd_attr = terminal_cmd.replace("'", "\\'").replace('"', '&quot;')

    # Introspect tables for preview
    tables = introspect_schema_summary(target)
    table_rows_html = ""
    if tables:
        for t in tables:
            table_rows_html += f"""
            <tr style="border-bottom: 1px solid rgba(255,255,255,0.06);">
                <td style="padding: 6px 10px; font-weight: 500;">{t['name']}</td>
                <td style="padding: 6px 10px; opacity: 0.7;">{t['schema']}</td>
                <td style="padding: 6px 10px; text-align: right; font-family: monospace;">{t['columns']}</td>
                <td style="padding: 6px 10px; text-align: right; font-family: monospace;">{t['rows']}</td>
            </tr>
            """
        preview_table_html = f"""
        <div style="margin-top: 14px; max-height: 180px; overflow-y: auto; border: 1px solid rgba(255,255,255,0.08); border-radius: 6px;">
            <table style="width: 100%; border-collapse: collapse; font-size: 12px; text-align: left;">
                <thead>
                    <tr style="background: rgba(255,255,255,0.04); border-bottom: 1px solid rgba(255,255,255,0.1); font-weight: 600; opacity: 0.85;">
                        <th style="padding: 6px 10px;">Table</th>
                        <th style="padding: 6px 10px;">Schema</th>
                        <th style="padding: 6px 10px; text-align: right;">Cols</th>
                        <th style="padding: 6px 10px; text-align: right;">Rows</th>
                    </tr>
                </thead>
                <tbody>
                    {table_rows_html}
                </tbody>
            </table>
        </div>
        """
    else:
        preview_table_html = "<div style='margin-top: 12px; font-size: 12px; opacity: 0.6;'>No user tables defined in connection.</div>"

    bridge_status_html = ""
    if is_in_memory_bridge:
        bridge_status_html = f"""
        <div style="display: flex; align-items: center; gap: 8px; margin-top: 12px; font-size: 11px; opacity: 0.8;">
            <span style="display: inline-block; width: 7px; height: 7px; border-radius: 50%; background: #10b981;"></span>
            <span>Live Python session bridged on <code>{db_arg}</code></span>
            <span>•</span>
            <span>Stop with <code>duckpack.stop_server()</code></span>
        </div>
        """

    title = "DuckPack Database Explorer" if mode == "explore" else "DuckPack Diff & Apply"
    action_desc = "launch the full-screen terminal IDE" if mode == "explore" else "review diffs and apply migrations interactively"

    html_content = f"""
    <div style="
        font-family: var(--vscode-font-family, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif);
        background: var(--vscode-editor-background, #161b22);
        color: var(--vscode-editor-foreground, #e6edf3);
        border: 1px solid var(--vscode-widget-border, rgba(255, 255, 255, 0.12));
        border-radius: 10px;
        padding: 16px 20px;
        margin: 10px 0;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
        max-width: 740px;
    ">
        <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px;">
            <div style="display: flex; align-items: center; gap: 10px;">
                <span style="font-size: 22px;">🦆</span>
                <div>
                    <div style="font-weight: 700; font-size: 15px; letter-spacing: -0.2px;">{title}</div>
                    <div style="font-size: 12px; opacity: 0.7;">Dev Container / Notebook Environment</div>
                </div>
            </div>
            <span style="background: rgba(245, 158, 11, 0.15); color: #f59e0b; border: 1px solid rgba(245, 158, 11, 0.3); font-size: 11px; font-weight: 600; padding: 3px 8px; border-radius: 12px;">v0.2.0</span>
        </div>

        <div style="background: rgba(56, 189, 248, 0.08); border: 1px solid rgba(56, 189, 248, 0.25); border-radius: 6px; padding: 10px 14px; font-size: 12px; line-height: 1.5; margin-bottom: 14px; color: #bae6fd;">
            ℹ️ <strong>Automatic terminal pop-out is disabled</strong> by Dev Container and VS Code notebook sandbox restrictions. To {action_desc}, paste the command below into your VS Code terminal panel:
        </div>

        <div>
            <div style="display: flex; align-items: center; justify-content: space-between; background: var(--vscode-terminal-background, #0d1117); border: 1px solid rgba(255, 255, 255, 0.12); border-radius: 6px; padding: 10px 14px;">
                <code style="font-family: var(--vscode-editor-font-family, monospace); font-size: 12.5px; color: #38bdf8; word-break: break-all; user-select: all;">$ {terminal_cmd}</code>
                <button onclick="navigator.clipboard.writeText('{safe_cmd_attr}').then(() => {{ this.innerText = '✓ Copied!'; setTimeout(() => this.innerText = '📋 Copy', 2000); }})" style="
                    background: rgba(255, 255, 255, 0.08);
                    border: 1px solid rgba(255, 255, 255, 0.16);
                    color: #e6edf3;
                    padding: 5px 12px;
                    border-radius: 4px;
                    font-size: 11px;
                    font-weight: 600;
                    cursor: pointer;
                    white-space: nowrap;
                    margin-left: 14px;
                ">📋 Copy</button>
            </div>
        </div>

        <div style="margin-top: 10px; font-size: 11px; opacity: 0.75; line-height: 1.4;">
            💡 <strong>Pro-Tip:</strong> In VS Code, right-click your terminal tab and choose <em>"Move Terminal into Editor Area"</em> to run DuckPack side-by-side with your code in a dedicated full-size editor workspace!
        </div>

        {bridge_status_html}
        {preview_table_html}
    </div>
    """

    display(HTML(html_content))
    return True
