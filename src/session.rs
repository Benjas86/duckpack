use anyhow::{Context, Result};
use std::path::Path;

use crate::diff as schema_diff;
use crate::engine::{self, DbExecutor, TargetSpec};
use crate::sql_utils::{quote_ident, quote_literal};
use crate::workspace::WorkspaceApplyReport;

/// RAII Guard that ensures terminal raw mode, alternate screen, and mouse capture are restored
/// even if an error, panic, or `KeyboardInterrupt` occurs during an interactive session.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> Result<Self> {
        ratatui::crossterm::terminal::enable_raw_mode().map_err(|e| {
            anyhow::anyhow!(
                "Failed to initialize terminal (raw mode): {}. \
                Interactive full-screen TUI requires an interactive terminal (TTY) and cannot run \
                inside non-TTY environments (such as Jupyter notebooks, web cells, or piped stdout).",
                e
            )
        })?;
        let mut stdout = std::io::stdout();
        ratatui::crossterm::execute!(
            stdout,
            ratatui::crossterm::terminal::EnterAlternateScreen,
            ratatui::crossterm::event::EnableMouseCapture
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = ratatui::crossterm::terminal::disable_raw_mode();
        let _ = ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::LeaveAlternateScreen,
            ratatui::crossterm::event::DisableMouseCapture
        );
    }
}

/// Summary of an automated multi-catalog `apply` pass executed over `&mut dyn DbExecutor`.
#[derive(Debug, Clone, Default)]
pub struct CatalogApplyReport {
    pub applied_items: Vec<String>,
    pub catalog_count: usize,
}

/// Applies declarative project changes across one or more target catalogs over any `DbExecutor`.
///
/// Threads `allow_lake_rewrite` explicitly into `engine::apply_diff_with_lake_options` so callers
/// never mutate process-global environment variables (fixes M3 sticky-state across calls).
#[allow(clippy::too_many_arguments)]
pub fn apply_catalogs_with_executor(
    exec: &mut dyn DbExecutor,
    project_dir: &Path,
    requested_catalogs: &[String],
    force_drop: bool,
    allow_lake_rewrite: bool,
    is_remote: bool,
    verbose_stdout: bool,
) -> Result<CatalogApplyReport> {
    let catalogs_to_apply = if requested_catalogs.is_empty() {
        let active = schema_diff::validate_target_catalog_exec(exec, None, true)?;
        vec![active]
    } else {
        requested_catalogs.to_vec()
    };

    let mut applied_items = Vec::new();
    for target_cat in &catalogs_to_apply {
        let active_catalog =
            schema_diff::validate_target_catalog_exec(exec, Some(target_cat), true)?;
        let diff_result =
            schema_diff::diff_project(project_dir, exec, Some(&active_catalog))
                .with_context(|| format!("Diff failed for catalog '{}'", active_catalog))?;

        let pending_items: Vec<&schema_diff::DiffItem> = diff_result
            .items
            .iter()
            .filter(|i| i.selected && !i.ignored && (!i.is_destructive() || force_drop))
            .collect();

        if pending_items.is_empty() {
            if verbose_stdout {
                println!(
                    "Target catalog '{}' is up to date (0 pending changes).",
                    active_catalog
                );
            }
            continue;
        }

        engine::apply_diff_with_lake_options(
            exec,
            &diff_result,
            force_drop,
            allow_lake_rewrite,
            is_remote,
            project_dir,
            Some(&active_catalog),
        )
        .with_context(|| format!("Apply error for catalog '{}'", active_catalog))?;

        let count = pending_items.len();
        for item in pending_items {
            applied_items.push(item.obj_name.clone());
        }
        if verbose_stdout {
            println!(
                "✓ Successfully applied {} change(s) automatically to catalog '{}'!",
                count, active_catalog
            );
        }
    }

    Ok(CatalogApplyReport {
        applied_items,
        catalog_count: catalogs_to_apply.len(),
    })
}

/// Formats a non-TTY / notebook database summary over any `DbExecutor` (`duckdb::Connection` or `PyDbExecutor`).
pub fn format_non_terminal_summary(
    exec: &mut dyn DbExecutor,
    db_label: Option<&str>,
    catalog: Option<&str>,
) -> Result<String> {
    let snapshot = schema_diff::CatalogSnapshot::introspect(exec, catalog)?;
    let table_map = snapshot.tables;
    let view_map = snapshot.views;
    let macro_map = snapshot.macros;

    // Discover temporary tables
    let mut temp_tables: Vec<(String, Vec<(String, String)>)> = Vec::new();
    if let Ok((_, rows)) = exec.query_rows(
        "SELECT table_name FROM duckdb_tables() WHERE temporary = true AND internal = false ORDER BY table_name",
    ) {
        for r in rows {
            if let Some(tname) = r.into_iter().next() {
                let mut cols = Vec::new();
                let col_sql = format!(
                    "SELECT column_name, data_type FROM duckdb_columns() WHERE database_name = 'temp' AND table_name = {} ORDER BY column_index",
                    quote_literal(&tname)
                );
                if let Ok((_, col_rows)) = exec.query_rows(&col_sql) {
                    for crow in col_rows {
                        if crow.len() >= 2 {
                            cols.push((crow[0].clone(), crow[1].clone()));
                        }
                    }
                }
                temp_tables.push((tname, cols));
            }
        }
    }

    // Discover temporary views
    let mut temp_views: Vec<String> = Vec::new();
    if let Ok((_, rows)) = exec.query_rows(
        "SELECT view_name FROM duckdb_views() WHERE temporary = true AND internal = false ORDER BY view_name",
    ) {
        for r in rows {
            if let Some(vname) = r.into_iter().next() {
                temp_views.push(vname);
            }
        }
    }

    let mut out = String::new();
    out.push_str("======================================================================\n");
    match (db_label, catalog) {
        (Some(db), Some(cat)) => {
            out.push_str(&format!(
                "🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: {} [catalog '{}'])\n",
                db, cat
            ));
        }
        (Some(db), None) => {
            out.push_str(&format!(
                "🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: {})\n",
                db
            ));
        }
        (None, Some(cat)) => {
            out.push_str(&format!(
                "🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: catalog '{}')\n",
                cat
            ));
        }
        (None, None) => {
            out.push_str("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode)\n");
        }
    }
    out.push_str("======================================================================\n");

    let total_tables = table_map.len() + temp_tables.len();
    if total_tables == 0 {
        out.push_str("📊 Tables: (none)\n");
    } else {
        out.push_str(&format!("📊 Tables ({}):\n", total_tables));
        let mut sorted_tables: Vec<_> = table_map.iter().collect();
        sorted_tables.sort_by_key(|(name, _)| (*name).clone());
        for (name, schema) in sorted_tables {
            let quoted_name = schema_diff::quote_qualified_identifier(name);
            let row_count = exec
                .query_rows(&format!("SELECT count(*) FROM {}", quoted_name))
                .ok()
                .and_then(|(_, rows)| rows.into_iter().next())
                .and_then(|r| r.into_iter().next())
                .unwrap_or_else(|| "?".to_string());

            out.push_str(&format!("  • {} ({} rows)\n", name, row_count));
            let mut sorted_cols: Vec<_> = schema.columns.iter().collect();
            sorted_cols.sort_by_key(|(col, _)| (*col).clone());
            for (col, dtype) in sorted_cols {
                out.push_str(&format!("    - {}: {}\n", col, dtype));
            }
        }

        for (tname, cols) in &temp_tables {
            let row_count = exec
                .query_rows(&format!("SELECT count(*) FROM {}", quote_ident(tname)))
                .ok()
                .and_then(|(_, rows)| rows.into_iter().next())
                .and_then(|r| r.into_iter().next())
                .unwrap_or_else(|| "?".to_string());

            out.push_str(&format!("  • {} [temp] ({} rows)\n", tname, row_count));
            for (col, dtype) in cols {
                out.push_str(&format!("    - {}: {}\n", col, dtype));
            }
        }
    }

    let total_views = view_map.len() + temp_views.len();
    if total_views > 0 {
        out.push_str(&format!("\n🔍 Views ({}):\n", total_views));
        for name in view_map.keys() {
            out.push_str(&format!("  • {}\n", name));
        }
        for name in &temp_views {
            out.push_str(&format!("  • {} [temp]\n", name));
        }
    }

    if !macro_map.is_empty() {
        out.push_str(&format!("\n🔧 Macros ({}):\n", macro_map.len()));
        for name in macro_map.keys() {
            out.push_str(&format!("  • {}\n", name));
        }
    }

    out.push_str("======================================================================\n");
    out.push_str("💡 Note: Full interactive TUI (multi-tab SQL editor, autocompletion,\n");
    out.push_str("   ERD diagram) requires an interactive terminal (TTY).\n");
    out.push_str("   To launch the interactive IDE, run your script from a terminal shell:\n");
    out.push_str(
        "   $ python -c \"import duckdb, duckpack; con = duckdb.connect(...); duckpack.explore(con)\"\n",
    );
    out.push_str("======================================================================");

    Ok(out)
}

/// Prepared connection + workspace attachment state for launching the IDE or non-TTY explorer.
pub struct OpenedExploreSession {
    pub conn: duckdb::Connection,
    pub display_db: String,
    pub active_catalog: Option<String>,
    pub ws_report: Option<WorkspaceApplyReport>,
}

/// Unified workspace-mode vs primary-target boot for CLI `duckpack explore` and Python `duckpack.explore()`.
pub fn open_explore_session(
    project_dir: &Path,
    env: Option<&str>,
    spec: &TargetSpec,
    explicit_db: Option<&str>,
    requested_catalog: Option<&str>,
    cleanup_quack_bridges: bool,
) -> Result<OpenedExploreSession> {
    let use_workspace_mode =
        crate::workspace::should_use_workspace_mode(project_dir, env, explicit_db);

    if use_workspace_mode {
        let conn = engine::open_duckdb_connection()?;
        let report = crate::workspace::apply_workspace_attachments(&conn, project_dir, env)?;
        let active_catalog = requested_catalog
            .map(String::from)
            .or_else(|| engine::resolve_project_catalog(project_dir, env))
            .or_else(|| report.default_alias.clone());
        if let Some(ref cat) = active_catalog {
            let _ = conn.execute_batch(&format!("USE {};", quote_ident(cat)));
        }
        let display_db = match env {
            Some(env_name) => format!("workspace [{}] (:memory:)", env_name),
            None => "workspace (:memory:)".to_string(),
        };
        Ok(OpenedExploreSession {
            conn,
            display_db,
            active_catalog,
            ws_report: Some(report),
        })
    } else {
        let conn = spec.connect()?;
        let report = crate::workspace::apply_workspace_attachments(&conn, project_dir, env)?;
        let active_catalog = requested_catalog
            .map(String::from)
            .or_else(|| engine::resolve_project_catalog(project_dir, env));
        if let Some(ref cat) = active_catalog {
            let _ = conn.execute_batch(&format!("USE {};", quote_ident(cat)));
        }
        if cleanup_quack_bridges && (spec.is_quack || spec.db.starts_with("quack:")) {
            engine::cleanup_orphaned_quack_bridges(&conn);
        }
        Ok(OpenedExploreSession {
            conn,
            display_db: spec.db.clone(),
            active_catalog,
            ws_report: Some(report),
        })
    }
}

/// Validates an SSH/SCP remote target string (`host`, `user@host`, `user@[::1]`) against
/// OpenSSH option injection (leading `-`) and shell/control-character injection (H2).
pub fn validate_safe_ssh_remote(remote: &str) -> Result<()> {
    let trimmed = remote.trim();
    if trimmed.is_empty() {
        anyhow::bail!("SSH remote target cannot be empty.");
    }
    if trimmed != remote {
        anyhow::bail!("SSH remote target '{}' must not contain leading or trailing whitespace.", remote);
    }
    if trimmed.starts_with('-') {
        anyhow::bail!(
            "Invalid SSH remote target '{}': target must not start with '-' (OpenSSH option injection guard).",
            remote
        );
    }
    if trimmed.chars().any(|c| {
        c.is_control()
            || c.is_whitespace()
            || matches!(c, ';' | '|' | '&' | '`' | '$' | '(' | ')' | '<' | '>' | '\'' | '"' | '\\' | '/')
    }) {
        anyhow::bail!(
            "Invalid SSH remote target '{}': contains forbidden whitespace, path, or shell metacharacters.",
            remote
        );
    }
    // If user@host, ensure neither side is empty and host doesn't start with '-'
    if let Some((user, host)) = trimmed.split_once('@') {
        if user.is_empty() || host.is_empty() || host.starts_with('-') || host.contains('@') {
            anyhow::bail!("Invalid SSH remote target '{}': expected 'host' or 'user@host'.", remote);
        }
    }
    Ok(())
}

/// Quotes a string as a single-quoted POSIX shell token (`'...'`, escaping embedded `'` as `'\''`).
pub fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;

    #[test]
    fn test_validate_safe_ssh_remote_rejects_option_and_shell_injection() {
        assert!(validate_safe_ssh_remote("prod.example.com").is_ok());
        assert!(validate_safe_ssh_remote("deploy@10.0.0.5").is_ok());
        assert!(validate_safe_ssh_remote("admin@[2001:db8::1]").is_ok());
        assert!(validate_safe_ssh_remote("db_user@my-host_01.internal").is_ok());

        // H2: OpenSSH option injection & command/shell injection
        assert!(validate_safe_ssh_remote("-oProxyCommand=id").is_err());
        assert!(validate_safe_ssh_remote("user@-oProxyCommand=id").is_err());
        assert!(validate_safe_ssh_remote("user@host;rm -rf /").is_err());
        assert!(validate_safe_ssh_remote("user@host\nwhoami").is_err());
        assert!(validate_safe_ssh_remote("user@host $(id)").is_err());
        assert!(validate_safe_ssh_remote("").is_err());
        assert!(validate_safe_ssh_remote("@host").is_err());
        assert!(validate_safe_ssh_remote("user@").is_err());
    }

    #[test]
    fn test_format_non_terminal_summary_over_db_executor() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (id INT, name VARCHAR);
             INSERT INTO users VALUES (1, 'Alice'), (2, 'Bob');
             CREATE TEMP TABLE tmp_scratch (x INT);
             INSERT INTO tmp_scratch VALUES (42);
             CREATE VIEW v_users AS SELECT * FROM users;
             CREATE TEMP VIEW v_tmp AS SELECT * FROM tmp_scratch;",
        )
        .unwrap();

        let summary =
            format_non_terminal_summary(&mut conn, Some(":memory:"), Some("memory")).unwrap();
        assert!(summary.contains(":memory: [catalog 'memory']"), "{summary}");
        assert!(summary.contains("users (2 rows)"), "{summary}");
        assert!(summary.contains("tmp_scratch [temp] (1 rows)"), "{summary}");
        assert!(summary.contains("v_users"), "{summary}");
        assert!(summary.contains("v_tmp [temp]"), "{summary}");
    }

    #[test]
    fn test_apply_catalogs_with_executor_threads_allow_lake_rewrite_without_env_mutation() {
        let dir = std::env::temp_dir().join(format!(
            "dp_session_apply_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::project::init_project(&dir).unwrap();
        std::fs::write(
            dir.join("tables").join("events.sql"),
            "CREATE TABLE events (id INT, kind VARCHAR);\n",
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        let report = apply_catalogs_with_executor(
            &mut conn,
            &dir,
            &[],
            false,
            true,
            false,
            false,
        )
        .unwrap();
        assert_eq!(report.catalog_count, 1);
        assert!(report.applied_items.iter().any(|i| i.contains("events")));
        // M3: Calling apply with allow_lake_rewrite=true must NOT pollute process env
        assert!(
            std::env::var("DUCKPACK_ALLOW_LAKE_REWRITE").is_err(),
            "apply_catalogs_with_executor must not mutate DUCKPACK_ALLOW_LAKE_REWRITE env var"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
