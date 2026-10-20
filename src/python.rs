use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::diff as schema_diff;
use crate::engine::{self, DbExecutor};

create_exception!(duckpack, Error, PyException);
create_exception!(duckpack, SchemaError, Error);
create_exception!(duckpack, ExecutionError, Error);

/// Wrapper around Python's DuckDB connection object implementing `DbExecutor`
pub struct PyDbExecutor {
    con: PyObject,
}

impl PyDbExecutor {
    pub fn new(con: PyObject) -> Self {
        Self { con }
    }
}

impl DbExecutor for PyDbExecutor {
    fn execute(&mut self, sql: &str) -> anyhow::Result<()> {
        Python::with_gil(|py| -> anyhow::Result<()> {
            self.con
                .call_method1(py, "execute", (sql,))
                .map_err(|e| anyhow::anyhow!("DuckDB execution error: {}", e))?;
            Ok(())
        })
    }

    fn execute_batch(&mut self, sql: &str) -> anyhow::Result<()> {
        self.execute(sql)
    }

    fn query_rows(&mut self, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<String>>)> {
        Python::with_gil(|py| -> anyhow::Result<(Vec<String>, Vec<Vec<String>>)> {
            let res = self
                .con
                .call_method1(py, "execute", (sql,))
                .map_err(|e| anyhow::anyhow!("DuckDB query error: {}", e))?;

            let mut col_names = Vec::new();
            if let Ok(desc) = res.getattr(py, "description") {
                if let Ok(desc_list) = desc.extract::<Vec<Bound<'_, PyAny>>>(py) {
                    for item in desc_list {
                        if let Ok(col_tuple) = item.extract::<(String, Bound<'_, PyAny>)>() {
                            col_names.push(col_tuple.0);
                        } else if let Ok(first) = item.get_item(0) {
                            if let Ok(name) = first.extract::<String>() {
                                col_names.push(name);
                            }
                        }
                    }
                }
            }

            let rows_obj = res
                .call_method0(py, "fetchall")
                .map_err(|e| anyhow::anyhow!("DuckDB fetchall error: {}", e))?;

            let mut formatted_rows = Vec::new();
            if let Ok(rows) = rows_obj.extract::<Vec<Bound<'_, PyAny>>>(py) {
                for row in rows {
                    let mut row_data = Vec::new();
                    if let Ok(cells) = row.extract::<Vec<Bound<'_, PyAny>>>() {
                        for cell in cells {
                            if cell.is_none() {
                                row_data.push("NULL".to_string());
                            } else {
                                row_data.push(cell.str().map(|s| s.to_string()).unwrap_or_else(|_| format!("{:?}", cell)));
                            }
                        }
                    }
                    formatted_rows.push(row_data);
                }
            }

            Ok((col_names, formatted_rows))
        })
    }
}

/// RAII Guard that ensures terminal raw mode and alternate screen are restored
/// even if a panic or KeyboardInterrupt occurs during an interactive session.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn enter() -> anyhow::Result<Self> {
        ratatui::crossterm::terminal::enable_raw_mode().map_err(|e| {
            anyhow::anyhow!(
                "Failed to initialize terminal (raw mode): {}. \
                duckpack.explore() requires an interactive terminal (TTY). \
                Interactive full-screen TUI cannot run inside non-TTY environments \
                (such as Jupyter notebooks, web cells, or piped stdout). \
                Please run from a terminal shell.",
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

fn is_interactive_terminal() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9494)
}

fn print_non_terminal_explorer(executor: &mut PyDbExecutor, catalog: Option<&str>) -> PyResult<()> {
    let (_schemas, table_map, view_map, macro_map) = introspect_py_catalog(executor, catalog)?;

    // Discover temporary tables
    let mut temp_tables: Vec<(String, Vec<(String, String)>)> = Vec::new();
    if let Ok((_, rows)) = executor.query_rows("SELECT table_name FROM duckdb_tables() WHERE temporary = true AND internal = false ORDER BY table_name") {
        for r in rows {
            if let Some(tname) = r.into_iter().next() {
                let mut cols = Vec::new();
                if let Ok((_, col_rows)) = executor.query_rows(&format!(
                    "SELECT column_name, data_type FROM duckdb_columns() WHERE database_name = 'temp' AND table_name = '{}' ORDER BY column_index",
                    tname.replace('\'', "''")
                )) {
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
    if let Ok((_, rows)) = executor.query_rows("SELECT view_name FROM duckdb_views() WHERE temporary = true AND internal = false ORDER BY view_name") {
        for r in rows {
            if let Some(vname) = r.into_iter().next() {
                temp_views.push(vname);
            }
        }
    }

    println!("======================================================================");
    if let Some(cat) = catalog {
        println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: catalog '{}')", cat);
    } else {
        println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode)");
    }
    println!("======================================================================");

    let total_tables = table_map.len() + temp_tables.len();
    if total_tables == 0 {
        println!("📊 Tables: (none)");
    } else {
        println!("📊 Tables ({}):", total_tables);
        let mut sorted_tables: Vec<_> = table_map.iter().collect();
        sorted_tables.sort_by_key(|(name, _)| (*name).clone());
        for (name, schema) in sorted_tables {
            let row_count = executor
                .query_rows(&format!("SELECT count(*) FROM {}", name))
                .ok()
                .and_then(|(_, rows)| rows.into_iter().next())
                .and_then(|r| r.into_iter().next())
                .unwrap_or_else(|| "?".to_string());

            println!("  • {} ({} rows)", name, row_count);
            let mut sorted_cols: Vec<_> = schema.columns.iter().collect();
            sorted_cols.sort_by_key(|(col, _)| (*col).clone());
            for (col, dtype) in sorted_cols {
                println!("    - {}: {}", col, dtype);
            }
        }

        for (tname, cols) in &temp_tables {
            let row_count = executor
                .query_rows(&format!("SELECT count(*) FROM \"{}\"", tname.replace('\"', "\"\"")))
                .ok()
                .and_then(|(_, rows)| rows.into_iter().next())
                .and_then(|r| r.into_iter().next())
                .unwrap_or_else(|| "?".to_string());

            println!("  • {} [temp] ({} rows)", tname, row_count);
            for (col, dtype) in cols {
                println!("    - {}: {}", col, dtype);
            }
        }
    }

    let total_views = view_map.len() + temp_views.len();
    if total_views > 0 {
        println!("\n🔍 Views ({}):", total_views);
        for name in view_map.keys() {
            println!("  • {}", name);
        }
        for name in &temp_views {
            println!("  • {} [temp]", name);
        }
    }

    if !macro_map.is_empty() {
        println!("\n🔧 Macros ({}):", macro_map.len());
        for name in macro_map.keys() {
            println!("  • {}", name);
        }
    }

    println!("======================================================================");
    println!("💡 Note: Full interactive TUI (multi-tab SQL editor, autocompletion,");
    println!("   ERD diagram) requires an interactive terminal (TTY).");
    println!("   To launch the interactive IDE, run your script from a terminal shell:");
    println!("   $ python -c \"import duckdb, duckpack; con = duckdb.connect(...); duckpack.explore(con)\"");
    println!("======================================================================");

    Ok(())
}

fn print_non_terminal_db(conn: &duckdb::Connection, db_name: &str, catalog: Option<&str>) -> PyResult<()> {
    let tables = schema_diff::get_schema(conn, catalog).unwrap_or_default();
    let views = schema_diff::get_views(conn, catalog).unwrap_or_default();
    let macros = schema_diff::get_macros(conn, catalog).unwrap_or_default();

    // Discover temporary tables
    let mut temp_tables: Vec<(String, Vec<(String, String)>)> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT table_name FROM duckdb_tables() WHERE temporary = true AND internal = false ORDER BY table_name") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(tname) = row.get::<_, String>(0) {
                    let mut cols = Vec::new();
                    let col_sql = format!(
                        "SELECT column_name, data_type FROM duckdb_columns() WHERE database_name = 'temp' AND table_name = '{}' ORDER BY column_index",
                        tname.replace('\'', "''")
                    );
                    if let Ok(mut cstmt) = conn.prepare(&col_sql) {
                        if let Ok(mut crows) = cstmt.query([]) {
                            while let Ok(Some(crow)) = crows.next() {
                                if let (Ok(cname), Ok(ctype)) = (crow.get::<_, String>(0), crow.get::<_, String>(1)) {
                                    cols.push((cname, ctype));
                                }
                            }
                        }
                    }
                    temp_tables.push((tname, cols));
                }
            }
        }
    }

    // Discover temporary views
    let mut temp_views: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT view_name FROM duckdb_views() WHERE temporary = true AND internal = false ORDER BY view_name") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(vname) = row.get::<_, String>(0) {
                    temp_views.push(vname);
                }
            }
        }
    }

    println!("======================================================================");
    if let Some(cat) = catalog {
        println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: {} [catalog '{}'])", db_name, cat);
    } else {
        println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: {})", db_name);
    }
    println!("======================================================================");

    let total_tables = tables.len() + temp_tables.len();
    if total_tables == 0 {
        println!("📊 Tables: (none)");
    } else {
        println!("📊 Tables ({}):", total_tables);
        let mut sorted_tables: Vec<_> = tables.iter().collect();
        sorted_tables.sort_by_key(|(name, _)| (*name).clone());
        for (name, schema) in sorted_tables {
            let row_count: String = conn
                .query_row(&format!("SELECT count(*) FROM {}", name), [], |r| {
                    let c: i64 = r.get(0)?;
                    Ok(c.to_string())
                })
                .unwrap_or_else(|_| "?".to_string());

            println!("  • {} ({} rows)", name, row_count);
            let mut sorted_cols: Vec<_> = schema.columns.iter().collect();
            sorted_cols.sort_by_key(|(col, _)| (*col).clone());
            for (col, dtype) in sorted_cols {
                println!("    - {}: {}", col, dtype);
            }
        }

        for (tname, cols) in &temp_tables {
            let row_count: String = conn
                .query_row(&format!("SELECT count(*) FROM \"{}\"", tname.replace('\"', "\"\"")), [], |r| {
                    let c: i64 = r.get(0)?;
                    Ok(c.to_string())
                })
                .unwrap_or_else(|_| "?".to_string());

            println!("  • {} [temp] ({} rows)", tname, row_count);
            for (col, dtype) in cols {
                println!("    - {}: {}", col, dtype);
            }
        }
    }

    let total_views = views.len() + temp_views.len();
    if total_views > 0 {
        println!("\n🔍 Views ({}):", total_views);
        for name in views.keys() {
            println!("  • {}", name);
        }
        for name in &temp_views {
            println!("  • {} [temp]", name);
        }
    }

    if !macros.is_empty() {
        println!("\n🔧 Macros ({}):", macros.len());
        for name in macros.keys() {
            println!("  • {}", name);
        }
    }

    println!("======================================================================");
    println!("💡 Note: Full interactive TUI (multi-tab SQL editor, autocompletion,");
    println!("   ERD diagram) requires an interactive terminal (TTY).");
    println!("   To launch the interactive IDE, run your script from a terminal shell:");
    println!("   $ python -c \"import duckdb, duckpack; con = duckdb.connect(...); duckpack.explore(con)\"");
    println!("======================================================================");

    Ok(())
}

/// Structured result of schema changes applied via `duckpack.apply()`
#[pyclass]
#[derive(Debug, Clone)]
pub struct ApplyResult {
    #[pyo3(get)]
    pub success: bool,
    #[pyo3(get)]
    pub applied_count: usize,
    #[pyo3(get)]
    pub items: Vec<String>,
}

#[pymethods]
impl ApplyResult {
    #[new]
    #[pyo3(signature = (success = true, applied_count = 0, items = Vec::new()))]
    fn new(success: bool, applied_count: usize, items: Vec<String>) -> Self {
        Self { success, applied_count, items }
    }

    fn __repr__(&self) -> String {
        format!("<ApplyResult success={} applied_count={}>", self.success, self.applied_count)
    }
}

/// Structured summary of schema differences between project and database
#[pyclass]
#[derive(Debug, Clone)]
pub struct DiffSummary {
    #[pyo3(get)]
    pub has_changes: bool,
    #[pyo3(get)]
    pub change_count: usize,
    #[pyo3(get)]
    pub items: Vec<String>,
}

#[pymethods]
impl DiffSummary {
    #[new]
    #[pyo3(signature = (has_changes = false, change_count = 0, items = Vec::new()))]
    fn new(has_changes: bool, change_count: usize, items: Vec<String>) -> Self {
        Self { has_changes, change_count, items }
    }

    fn __repr__(&self) -> String {
        format!("<DiffSummary has_changes={} change_count={}>", self.has_changes, self.change_count)
    }
}

/// Initialize a new declarative DuckDB project scaffold.
#[pyfunction]
#[pyo3(signature = (project_dir = "."))]
pub fn init(project_dir: &str) -> PyResult<()> {
    engine::init_project(Path::new(project_dir))
        .map_err(|e| SchemaError::new_err(format!("Failed to initialize project: {}", e)))
}

/// Build and validate project syntax against an in-memory Shadow DB.
#[pyfunction]
#[pyo3(signature = (project_dir = "."))]
pub fn build(project_dir: &str) -> PyResult<()> {
    engine::build_shadow_db(Path::new(project_dir))
        .map_err(|e| SchemaError::new_err(format!("Build validation failed: {}", e)))?;
    Ok(())
}

/// Compile a declarative DuckDB project into an immutable `.duckpack` snapshot artifact.
#[pyfunction]
#[pyo3(signature = (project_dir, out))]
pub fn compile(project_dir: &str, out: &str) -> PyResult<()> {
    let p_dir = Path::new(project_dir);
    let _shadow = engine::build_shadow_db(p_dir)
        .map_err(|e| SchemaError::new_err(format!("Validation failed: {}", e)))?;

    let _ = std::fs::remove_file(out);
    let out_conn = duckdb::Connection::open(out)
        .map_err(|e| ExecutionError::new_err(format!("Failed to create .duckpack: {}", e)))?;

    let schemas_dir = p_dir.join("schemas");
    if schemas_dir.exists() {
        engine::execute_directory(&out_conn, &schemas_dir)
            .map_err(|e| SchemaError::new_err(format!("Schema compilation error: {}", e)))?;
    }
    let tables_dir = p_dir.join("tables");
    if tables_dir.exists() {
        engine::execute_directory(&out_conn, &tables_dir)
            .map_err(|e| SchemaError::new_err(format!("Table compilation error: {}", e)))?;
    }
    let views_dir = p_dir.join("views");
    if views_dir.exists() {
        engine::execute_directory(&out_conn, &views_dir)
            .map_err(|e| SchemaError::new_err(format!("View compilation error: {}", e)))?;
    }
    let macros_dir = p_dir.join("macros");
    if macros_dir.exists() {
        engine::execute_directory(&out_conn, &macros_dir)
            .map_err(|e| SchemaError::new_err(format!("Macro compilation error: {}", e)))?;
    }

    Ok(())
}

/// Compute differences between project declarative definitions and the target database.
#[pyfunction]
#[pyo3(signature = (target = None, project_dir = ".", database = None, catalog = None, quack_token = None, insecure = false, env = None))]
pub fn diff(
    _py: Python<'_>,
    target: Option<&Bound<'_, PyAny>>,
    project_dir: &str,
    database: Option<&str>,
    catalog: Option<&str>,
    quack_token: Option<&str>,
    insecure: bool,
    env: Option<&str>,
) -> PyResult<DiffSummary> {
    let p_dir = PathBuf::from(project_dir);
    let shadow_conn = engine::build_shadow_db(&p_dir)
        .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

    let shadow_schemas = schema_diff::get_schemas(&shadow_conn, None)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow schemas: {}", e)))?;
    let shadow_schema = schema_diff::get_schema(&shadow_conn, None)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow schema: {}", e)))?;
    let shadow_views = schema_diff::get_views(&shadow_conn, None)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow views: {}", e)))?;
    let shadow_macros = schema_diff::get_macros(&shadow_conn, None)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow macros: {}", e)))?;

    let explicit_db = target.and_then(|t| t.extract::<String>().ok());
    let explicit_catalog = database.or(catalog).map(|s| vec![s.to_string()]);
    let spec = engine::TargetResolver::resolve(
        &p_dir,
        env,
        explicit_db.as_deref(),
        quack_token,
        Some(insecure),
        explicit_catalog,
    );
    let req_catalog = spec.catalogs.first().cloned();

    // Determine target connection
    let (target_schemas, target_schema, target_views, target_macros) = if target.is_none() || explicit_db.is_some() {
        let conn = spec.connect()
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
        let active_cat = schema_diff::validate_target_catalog(&conn, req_catalog.as_deref(), false)
            .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        (
            schema_diff::get_schemas(&conn, Some(&active_cat)).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_schema(&conn, Some(&active_cat)).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_views(&conn, Some(&active_cat)).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_macros(&conn, Some(&active_cat)).map_err(|e| ExecutionError::new_err(e.to_string()))?,
        )
    } else {
        // Live Python connection: introspect catalog directly
        let t = target.unwrap();
        let mut executor = PyDbExecutor::new(t.clone().unbind());
        let active_cat = validate_py_target_catalog(&mut executor, req_catalog.as_deref(), false)?;
        introspect_py_catalog(&mut executor, Some(&active_cat))?
    };

    let ignore_list = engine::load_ignore_rules(&p_dir);

    let diff_result = schema_diff::compute_diff(
        &shadow_schemas,
        &target_schemas,
        &shadow_schema,
        &target_schema,
        &shadow_views,
        &target_views,
        &shadow_macros,
        &target_macros,
        &p_dir,
        &ignore_list,
    );

    let unignored_items: Vec<_> = diff_result
        .items
        .into_iter()
        .filter(|i| !i.ignored)
        .collect();
    let change_count = unignored_items.len();
    let items = unignored_items.iter().map(|item| format!("{:?}: {}", item.item_type, item.obj_name)).collect();

    Ok(DiffSummary {
        has_changes: change_count > 0,
        change_count,
        items,
    })
}

fn run_apply_tui(
    conn: &duckdb::Connection,
    project_dir: &PathBuf,
    db_target: &str,
    force_drop: bool,
    catalog: Option<&str>,
) -> PyResult<ApplyResult> {
    let _guard = TerminalGuard::enter()
        .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

    let stdout = std::io::stdout();
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)
        .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

    let mut active_catalog = schema_diff::validate_target_catalog(conn, catalog, true)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;

    let mut status_msg = String::from("Ready. Press 'Enter' to Apply, 'r' to Refresh, 'b' to switch catalog, 'q' to Quit.");
    let mut current_force_drop = force_drop;
    let mut inspect_data: Option<(String, Vec<String>, Vec<Vec<String>>, i64)> = None;
    let mut total_applied_count: usize = 0;
    let mut total_applied_items: Vec<String> = Vec::new();

    loop {
        let shadow_conn = engine::build_shadow_db(project_dir)
            .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

        let shadow_schemas = schema_diff::get_schemas(&shadow_conn, None)
            .map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_schemas = schema_diff::get_schemas(conn, Some(&active_catalog))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_schema = schema_diff::get_schema(&shadow_conn, None)
            .map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_schema = schema_diff::get_schema(conn, Some(&active_catalog))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_views = schema_diff::get_views(&shadow_conn, None)
            .map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_views = schema_diff::get_views(conn, Some(&active_catalog))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_macros = schema_diff::get_macros(&shadow_conn, None)
            .map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_macros = schema_diff::get_macros(conn, Some(&active_catalog))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let ignore_list = engine::load_ignore_rules(project_dir);

        let mut diff_result = schema_diff::compute_diff(
            &shadow_schemas,
            &target_schemas,
            &shadow_schema,
            &target_schema,
            &shadow_views,
            &target_views,
            &shadow_macros,
            &target_macros,
            project_dir,
            &ignore_list,
        );

        let display_target = if db_target.starts_with("quack:localhost:") || db_target.starts_with("quack:127.0.0.1:") {
            format!("{} (Python In-Memory)", db_target)
        } else {
            db_target.to_string()
        };
        let title = format!("{} [{}]", display_target, active_catalog);
        let action = crate::tui::draw_and_handle_events(
            &mut terminal,
            &mut diff_result,
            current_force_drop,
            &status_msg,
            false,
            &inspect_data,
            &title,
        ).map_err(|e| ExecutionError::new_err(format!("TUI error: {}", e)))?;

        match action {
            crate::tui::TuiAction::Quit => {
                return Ok(ApplyResult {
                    success: total_applied_count > 0,
                    applied_count: total_applied_count,
                    items: total_applied_items,
                });
            }
            crate::tui::TuiAction::SwitchCatalog => {
                let attached = schema_diff::list_attached_databases(conn).unwrap_or_default();
                let writable_dbs: Vec<_> = attached
                    .into_iter()
                    .filter(|d| !d.readonly && (d.db_type.eq_ignore_ascii_case("duckdb") || d.db_type.eq_ignore_ascii_case("quack")))
                    .collect();
                if writable_dbs.len() > 1 {
                    let curr_idx = writable_dbs.iter().position(|d| d.name.eq_ignore_ascii_case(&active_catalog)).unwrap_or(0);
                    let next_idx = (curr_idx + 1) % writable_dbs.len();
                    active_catalog = writable_dbs[next_idx].name.clone();
                    status_msg = format!("Switched target catalog to '{}'", active_catalog);
                } else {
                    status_msg = format!("Catalog '{}' is the only writable DuckDB database attached.", active_catalog);
                }
                continue;
            }
            crate::tui::TuiAction::CopyStatus => {
                let _ = std::fs::write("duckpack.log", &status_msg);
                status_msg = "Status text copied to duckpack.log!".to_string();
                continue;
            }
            crate::tui::TuiAction::Refresh => {
                status_msg = "Refreshed schema from disk.".to_string();
                continue;
            }
            crate::tui::TuiAction::ToggleDestructiveMode => {
                current_force_drop = !current_force_drop;
                status_msg = if current_force_drop {
                    "Destructive Mode ENABLED. Drops will be executed.".to_string()
                } else {
                    "Destructive Mode DISABLED. Drops will be ignored.".to_string()
                };
                continue;
            }
            crate::tui::TuiAction::Inspect { obj_name } => {
                let safe_obj = obj_name.replace('\'', "''");
                let mut cols = Vec::new();
                if let Ok(mut pragma_stmt) = conn.prepare(&format!("PRAGMA table_info('{}')", safe_obj)) {
                    if let Ok(mut pragma_rows) = pragma_stmt.query([]) {
                        while let Ok(Some(row)) = pragma_rows.next() {
                            if let Ok(col_name) = row.get::<_, String>(1) {
                                cols.push(col_name);
                            }
                        }
                    }
                }

                if cols.is_empty() {
                    status_msg = format!("Cannot inspect '{}': Does not exist in target database yet.", obj_name);
                } else {
                    let mut row_count = 0;
                    if let Ok(mut count_stmt) = conn.prepare(&format!("SELECT count(*) FROM \"{}\"", safe_obj)) {
                        if let Ok(mut count_rows) = count_stmt.query([]) {
                            if let Ok(Some(row)) = count_rows.next() {
                                row_count = row.get::<_, i64>(0).unwrap_or(0);
                            }
                        }
                    }

                    let cast_cols = cols.iter().map(|c| format!("CAST(\"{}\" AS VARCHAR)", c)).collect::<Vec<_>>().join(", ");
                    if let Ok(mut stmt) = conn.prepare(&format!("SELECT {} FROM \"{}\" LIMIT 5", cast_cols, safe_obj)) {
                        if let Ok(mut db_rows) = stmt.query([]) {
                            let mut rows = Vec::new();
                            while let Ok(Some(row)) = db_rows.next() {
                                let mut str_row = Vec::new();
                                for i in 0..cols.len() {
                                    let val: String = row.get(i).unwrap_or_else(|_| "NULL".to_string());
                                    str_row.push(val);
                                }
                                rows.push(str_row);
                            }
                            inspect_data = Some((obj_name, cols, rows, row_count));
                        }
                    }
                }
                continue;
            }
            crate::tui::TuiAction::CloseInspect => {
                inspect_data = None;
                continue;
            }
            crate::tui::TuiAction::Explore => {
                let _ = crate::ide::run_ide_loop(&mut terminal, conn, project_dir, db_target, false);
                terminal.clear().ok();
                continue;
            }
            crate::tui::TuiAction::Pull { obj_name, item_type } => {
                let sql = match item_type {
                    crate::diff::DiffItemType::DropTable => {
                        target_schema.get(&obj_name).map(|t| t.create_sql.clone())
                    }
                    crate::diff::DiffItemType::DropView => target_views.get(&obj_name).cloned(),
                    crate::diff::DiffItemType::DropMacro => target_macros.get(&obj_name).cloned(),
                    _ => None,
                };
                if let Some(sql) = sql {
                    let mut path = project_dir.to_path_buf();
                    match item_type {
                        crate::diff::DiffItemType::DropTable => path.push("tables"),
                        crate::diff::DiffItemType::DropView => path.push("views"),
                        crate::diff::DiffItemType::DropMacro => path.push("macros"),
                        _ => {}
                    }
                    path.push(format!("{}.sql", obj_name));
                    if let Err(e) = std::fs::write(&path, sql) {
                        status_msg = format!("Error pulling '{}': {}", obj_name, e);
                    } else {
                        status_msg = format!("Successfully pulled '{}' back into local project!", obj_name);
                    }
                } else {
                    status_msg = format!("Cannot pull '{}': Definition not found in remote DB.", obj_name);
                }
                continue;
            }
            crate::tui::TuiAction::Apply => {
                status_msg = "Applying changes...".to_string();
                let _ = crate::tui::draw_and_handle_events(
                    &mut terminal,
                    &mut diff_result,
                    current_force_drop,
                    &status_msg,
                    true,
                    &inspect_data,
                    &title,
                );

                let remote = engine::is_remote(db_target);
                let applied_items: Vec<String> = diff_result
                    .items
                    .iter()
                    .filter(|i| i.selected && !i.ignored)
                    .map(|i| i.obj_name.clone())
                    .collect();
                let applied_count = applied_items.len();

                if !remote && !db_target.starts_with("quack:") {
                    let db_path = PathBuf::from(db_target);
                    let mut b_path = db_path.clone();
                    b_path.set_extension("duckdb.bak");
                    if db_path.exists() {
                        let _ = std::fs::copy(&db_path, &b_path);
                    }
                    let apply_res = engine::apply_diff(conn, &diff_result, current_force_drop, remote, project_dir, Some(&active_catalog));
                    match apply_res {
                        Ok(_) => {
                            if b_path.exists() {
                                std::fs::remove_file(&b_path).ok();
                            }
                            total_applied_count += applied_count;
                            total_applied_items.extend(applied_items);
                            status_msg = format!("✓ Successfully applied {} change(s) to '{}'! Press 'b' to switch catalog, or 'q' to finish.", applied_count, active_catalog);
                            continue;
                        }
                        Err(e) => {
                            if b_path.exists() {
                                let _ = std::fs::copy(&b_path, &db_path);
                                std::fs::remove_file(&b_path).ok();
                            }
                            status_msg = format!("Apply error: {}", e);
                            continue;
                        }
                    }
                } else {
                    let apply_res = engine::apply_diff(conn, &diff_result, current_force_drop, remote, project_dir, Some(&active_catalog));
                    match apply_res {
                        Ok(_) => {
                            total_applied_count += applied_count;
                            total_applied_items.extend(applied_items);
                            status_msg = format!("✓ Successfully applied {} change(s) to '{}'! Press 'b' to switch catalog, or 'q' to finish.", applied_count, active_catalog);
                            continue;
                        }
                        Err(e) => {
                            status_msg = format!("Apply error: {}", e);
                            continue;
                        }
                    }
                }
            }
        }
    }
}

/// Apply declarative schema migrations to the target database.
#[pyfunction]
#[pyo3(signature = (target = None, project_dir = ".", database = None, catalog = None, auto_approve = true, force_drop = false, interactive = false, quack_token = None, insecure = false, env = None))]
pub fn apply(
    _py: Python<'_>,
    target: Option<&Bound<'_, PyAny>>,
    project_dir: &str,
    database: Option<&str>,
    catalog: Option<&str>,
    auto_approve: bool,
    force_drop: bool,
    interactive: bool,
    quack_token: Option<&str>,
    insecure: bool,
    env: Option<&str>,
) -> PyResult<ApplyResult> {
    let p_dir = PathBuf::from(project_dir);
    if !p_dir.exists() {
        return Err(SchemaError::new_err(format!("Project directory {} does not exist", p_dir.display())));
    }

    let explicit_db = target.and_then(|t| t.extract::<String>().ok());
    let explicit_catalogs = database.or(catalog).map(|cat| vec![cat.to_string()]);
    let spec = engine::TargetResolver::resolve(
        &p_dir,
        env,
        explicit_db.as_deref(),
        quack_token,
        Some(insecure),
        explicit_catalogs,
    );
    let req_catalogs = spec.catalogs.clone();
    let ignore_list = engine::load_ignore_rules(&p_dir);

    if target.is_none() || explicit_db.is_some() {
        let conn = spec.connect()
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;

        if (!auto_approve || interactive) && is_interactive_terminal() {
            let start_cat = req_catalogs.first().map(|s| s.as_str());
            return run_apply_tui(&conn, &p_dir, &spec.db, force_drop, start_cat);
        } else if !auto_approve {
            return Err(ExecutionError::new_err(
                "Cannot apply schema changes: auto_approve is False in a non-interactive environment. \
                 Pass auto_approve=True to approve and apply migrations automatically.",
            ));
        }

        let catalogs_to_apply = if req_catalogs.is_empty() {
            let active = schema_diff::validate_target_catalog(&conn, None, true)
                .map_err(|e| ExecutionError::new_err(e.to_string()))?;
            vec![active]
        } else {
            req_catalogs
        };

        let remote = engine::is_remote(&spec.db);
        let mut total_applied_items = Vec::new();

        for target_cat in &catalogs_to_apply {
            let active_catalog = schema_diff::validate_target_catalog(&conn, Some(target_cat), true)
                .map_err(|e| ExecutionError::new_err(e.to_string()))?;

            let shadow_conn = engine::build_shadow_db(&p_dir)
                .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

            let shadow_schemas = schema_diff::get_schemas(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let target_schemas = schema_diff::get_schemas(&conn, Some(&active_catalog)).map_err(|e| ExecutionError::new_err(e.to_string()))?;
            let shadow_schema = schema_diff::get_schema(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let target_schema = schema_diff::get_schema(&conn, Some(&active_catalog)).map_err(|e| ExecutionError::new_err(e.to_string()))?;
            let shadow_views = schema_diff::get_views(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let target_views = schema_diff::get_views(&conn, Some(&active_catalog)).map_err(|e| ExecutionError::new_err(e.to_string()))?;
            let shadow_macros = schema_diff::get_macros(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let target_macros = schema_diff::get_macros(&conn, Some(&active_catalog)).map_err(|e| ExecutionError::new_err(e.to_string()))?;

            let diff_result = schema_diff::compute_diff(
                &shadow_schemas,
                &target_schemas,
                &shadow_schema,
                &target_schema,
                &shadow_views,
                &target_views,
                &shadow_macros,
                &target_macros,
                &p_dir,
                &ignore_list,
            );

            let pending_items: Vec<_> = diff_result
                .items
                .iter()
                .filter(|i| i.selected && !i.ignored)
                .collect();
            if pending_items.is_empty() {
                continue;
            }

            engine::apply_diff(&conn, &diff_result, force_drop, remote, &p_dir, Some(&active_catalog))
                .map_err(|e| ExecutionError::new_err(format!("Apply error for catalog '{}': {}", active_catalog, e)))?;

            for i in pending_items {
                total_applied_items.push(i.obj_name.clone());
            }
        }

        let applied_count = total_applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: total_applied_items,
        })
    } else {
        let target = target.unwrap();
        if (!auto_approve || interactive) && is_interactive_terminal() {
            let db_name = target
                .getattr("execute")
                .ok()
                .and_then(|_| {
                    target.call_method1("execute", ("PRAGMA database_list;",)).ok()
                })
                .and_then(|res| res.call_method0("fetchall").ok())
                .and_then(|rows| {
                    if let Ok(r) = rows.extract::<Vec<(Bound<'_, PyAny>, Bound<'_, PyAny>, String)>>() {
                        r.into_iter().next().map(|t| t.2)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| ":memory:".to_string());

            let (conn, active_db_target) = if !db_name.is_empty() && db_name != ":memory:" {
                match engine::connect_target_db(&db_name, None, false) {
                    Ok(c) => (c, db_name),
                    Err(_) => {
                        let port = find_available_port();
                        let start_server_sql = format!(
                            "INSTALL quack; LOAD quack; CALL quack_serve('quack:localhost:{}', token := 'duckpack_py');",
                            port
                        );
                        if let Err(serve_err) = target.call_method1("execute", (start_server_sql,)) {
                            return Err(ExecutionError::new_err(format!(
                                "Unable to start local quack bridge: {}", serve_err
                            )));
                        }
                        let server_uri = format!("quack:localhost:{}", port);
                        let c = engine::connect_target_db(&server_uri, Some("duckpack_py"), false)
                            .map_err(|e| ExecutionError::new_err(format!("Failed to attach to Python Quack bridge: {}", e)))?;
                        (c, server_uri)
                    }
                }
            } else {
                let port = find_available_port();
                let start_server_sql = format!(
                    "INSTALL quack; LOAD quack; CALL quack_serve('quack:localhost:{}', token := 'duckpack_py');",
                    port
                );
                if let Err(e) = target.call_method1("execute", (start_server_sql,)) {
                    return Err(ExecutionError::new_err(format!(
                        "Unable to start local quack bridge for in-memory DuckDB connection: {}. \
                        Tip: Ensure the 'quack' extension can be loaded.",
                        e
                    )));
                }
                let server_uri = format!("quack:localhost:{}", port);
                let c = engine::connect_target_db(&server_uri, Some("duckpack_py"), false)
                    .map_err(|e| ExecutionError::new_err(format!("Failed to attach to Python in-memory DuckDB: {}", e)))?;
                (c, server_uri)
            };

            let start_cat = req_catalogs.first().map(|s| s.as_str());
            let res = run_apply_tui(&conn, &p_dir, &active_db_target, force_drop, start_cat);
            if active_db_target.starts_with("quack:") {
                let _ = target.call_method1("execute", (format!("CALL quack_stop('{}');", active_db_target),));
            }
            return res;
        } else if !auto_approve {
            return Err(ExecutionError::new_err(
                "Cannot apply schema changes: auto_approve is False in a non-interactive environment. \
                 Pass auto_approve=True to approve and apply migrations automatically.",
            ));
        }

        let mut executor = PyDbExecutor::new(target.clone().unbind());
        let catalogs_to_apply = if req_catalogs.is_empty() {
            let active = validate_py_target_catalog(&mut executor, None, true)?;
            vec![active]
        } else {
            req_catalogs
        };

        let mut total_applied_items = Vec::new();

        for target_cat in &catalogs_to_apply {
            let active_catalog = validate_py_target_catalog(&mut executor, Some(target_cat), true)?;

            let shadow_conn = engine::build_shadow_db(&p_dir)
                .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

            let shadow_schemas = schema_diff::get_schemas(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let shadow_schema = schema_diff::get_schema(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let shadow_views = schema_diff::get_views(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;
            let shadow_macros = schema_diff::get_macros(&shadow_conn, None).map_err(|e| SchemaError::new_err(e.to_string()))?;

            let (target_schemas, target_schema, target_views, target_macros) = introspect_py_catalog(&mut executor, Some(&active_catalog))?;

            let diff_result = schema_diff::compute_diff(
                &shadow_schemas,
                &target_schemas,
                &shadow_schema,
                &target_schema,
                &shadow_views,
                &target_views,
                &shadow_macros,
                &target_macros,
                &p_dir,
                &ignore_list,
            );

            let pending_items: Vec<_> = diff_result
                .items
                .iter()
                .filter(|i| i.selected && !i.ignored)
                .collect();
            if pending_items.is_empty() {
                continue;
            }

            let orig_cat: Option<String> = executor
                .query_rows("SELECT current_database()")
                .ok()
                .and_then(|(_, r)| r.into_iter().next())
                .and_then(|r| r.into_iter().next());
            if let Some(ref orig) = orig_cat {
                if !orig.eq_ignore_ascii_case(&active_catalog) {
                    executor.execute(&format!("USE \"{}\";", active_catalog.replace('\"', "\"\"")))
                        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
                }
            }

            for item in &diff_result.items {
                if !item.selected || item.ignored { continue; }
                let is_drop = matches!(
                    item.item_type,
                    schema_diff::DiffItemType::DropTable
                        | schema_diff::DiffItemType::DropColumn
                        | schema_diff::DiffItemType::DropView
                        | schema_diff::DiffItemType::DropMacro
                        | schema_diff::DiffItemType::DropSchema
                );
                if is_drop && !force_drop {
                    continue;
                }
                if !item.sql.trim().is_empty() {
                    executor.execute(&item.sql)
                        .map_err(|e| ExecutionError::new_err(format!("Failed applying {}: {}", item.obj_name, e)))?;
                }
            }

            if let Some(ref orig) = orig_cat {
                if !orig.eq_ignore_ascii_case(&active_catalog) {
                    let _ = executor.execute(&format!("USE \"{}\";", orig.replace('\"', "\"\"")));
                }
            }

            for i in pending_items {
                total_applied_items.push(i.obj_name.clone());
            }
        }

        let applied_count = total_applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: total_applied_items,
        })
    }
}

/// Validate target catalog against duckdb_databases() on Python connection.
pub fn validate_py_target_catalog(
    executor: &mut PyDbExecutor,
    catalog: Option<&str>,
    for_apply: bool,
) -> PyResult<String> {
    let (_, db_rows) = executor
        .query_rows("SELECT database_name, type, readonly FROM duckdb_databases() WHERE internal = false")
        .map_err(|e| ExecutionError::new_err(format!("Failed to list databases: {}", e)))?;

    let target = match catalog {
        Some(c) => c.to_string(),
        None => {
            let (_, curr) = executor
                .query_rows("SELECT current_database()")
                .map_err(|e| ExecutionError::new_err(format!("Failed to get current database: {}", e)))?;
            curr.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_else(|| "memory".to_string())
        }
    };

    let mut found = None;
    let mut available = Vec::new();
    for row in &db_rows {
        if row.len() >= 3 {
            let name = &row[0];
            let typ = &row[1];
            let readonly = row[2].eq_ignore_ascii_case("true") || row[2] == "1";
            available.push(name.clone());
            if name.eq_ignore_ascii_case(&target) {
                found = Some((name.clone(), typ.clone(), readonly));
            }
        }
    }

    if let Some((name, typ, readonly)) = found {
        if for_apply {
            if readonly {
                return Err(ExecutionError::new_err(format!(
                    "Cannot apply migrations to database '{}': attached in READ_ONLY mode.",
                    name
                )));
            }
            let typ_lower = typ.to_lowercase();
            if typ_lower != "duckdb" && typ_lower != "quack" {
                return Err(ExecutionError::new_err(format!(
                    "Cannot apply declarative schema migrations to '{}' (engine type: {}). \
                    DuckPack declarative schema management is designed specifically for DuckDB catalogs. \
                    External attached engines are supported for querying and DML, but schema DDL is managed externally.",
                    name, typ
                )));
            }
        }
        Ok(name)
    } else if db_rows.is_empty() {
        Ok(target)
    } else {
        Err(ExecutionError::new_err(format!(
            "Attached database '{}' not found on connection. Available databases: {:?}",
            target, available
        )))
    }
}

/// Helper to introspect tables, views, and macros from a PyDbExecutor for a specific catalog
fn introspect_py_catalog(
    executor: &mut PyDbExecutor,
    catalog: Option<&str>,
) -> PyResult<(
    HashSet<String>,
    std::collections::HashMap<String, schema_diff::TableSchema>,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
)> {
    use std::collections::HashMap;

    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => {
            let (_, curr) = executor
                .query_rows("SELECT current_database()")
                .unwrap_or_default();
            curr.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or_else(|| "memory".to_string())
        }
    };
    let safe_cat = target_cat.replace('\'', "''");

    // 1. Schemas
    let schema_query = format!(
        "SELECT schema_name FROM duckdb_schemas() WHERE internal = false AND database_name = '{}' AND schema_name NOT IN ('information_schema', 'pg_catalog', 'main')",
        safe_cat
    );
    let (_, schema_rows) = executor
        .query_rows(&schema_query)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let schemas: HashSet<String> = schema_rows.into_iter().filter_map(|r| r.into_iter().next()).collect();

    // 2. Tables
    let table_query = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, COALESCE(sql, '') FROM duckdb_tables() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let (_, table_rows) = executor
        .query_rows(&table_query)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let mut table_map = HashMap::new();
    for row in table_rows {
        if row.len() >= 2 {
            table_map.insert(
                row[0].clone(),
                schema_diff::TableSchema {
                    create_sql: row[1].clone(),
                    columns: HashMap::new(),
                },
            );
        }
    }

    // 3. Columns
    let col_query = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, column_name, data_type FROM duckdb_columns() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let (_, col_rows) = executor
        .query_rows(&col_query)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    for row in col_rows {
        if row.len() >= 3 {
            if let Some(t) = table_map.get_mut(&row[0]) {
                t.columns.insert(row[1].clone(), row[2].clone());
            }
        }
    }

    // Check if target is an attached Quack database that needs remote introspect queries
    let is_quack = if let Ok((_, db_rows)) = executor.query_rows(&format!("SELECT type FROM duckdb_databases() WHERE internal = false AND database_name = '{}'", safe_cat)) {
        db_rows.into_iter().next().map(|r| r.get(0).map(|t| t.eq_ignore_ascii_case("quack")).unwrap_or(false)).unwrap_or(false)
    } else {
        false
    };

    if is_quack && table_map.is_empty() {
        let tbl_sql = format!(
            "SELECT '{}.' || (CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END), COALESCE(sql, '') FROM {}.query('SELECT schema_name, table_name, sql FROM duckdb_tables() WHERE internal = false')",
            safe_cat, safe_cat
        );
        if let Ok((_, q_tbls)) = executor.query_rows(&tbl_sql) {
            for row in q_tbls {
                if row.len() >= 2 {
                    table_map.insert(
                        row[0].clone(),
                        schema_diff::TableSchema {
                            create_sql: row[1].clone(),
                            columns: HashMap::new(),
                        },
                    );
                }
            }
        }

        let col_sql = format!(
            "SELECT '{}.' || (CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END), column_name, data_type FROM {}.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false')",
            safe_cat, safe_cat
        );
        if let Ok((_, q_cols)) = executor.query_rows(&col_sql) {
            for row in q_cols {
                if row.len() >= 3 {
                    if let Some(t) = table_map.get_mut(&row[0]) {
                        t.columns.insert(row[1].clone(), row[2].clone());
                    }
                }
            }
        }
    }

    // 4. Views
    let view_query = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, COALESCE(sql, '') FROM duckdb_views() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let (_, view_rows) = executor
        .query_rows(&view_query)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let mut view_map = HashMap::new();
    for row in view_rows {
        if row.len() >= 2 {
            view_map.insert(row[0].clone(), row[1].clone());
        }
    }

    // 5. Macros
    let macro_query = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END, macro_definition, CAST(parameters AS VARCHAR) FROM duckdb_functions() WHERE database_name = '{}' AND function_type = 'macro' AND internal = false",
        safe_cat
    );
    let (_, macro_rows) = executor
        .query_rows(&macro_query)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let mut macro_map = HashMap::new();
    for row in macro_rows {
        if row.len() >= 3 {
            let params_clean = row[2].trim_matches(|c| c == '[' || c == ']');
            let sql = format!("CREATE MACRO {}({}) AS {};", row[0], params_clean, row[1]);
            macro_map.insert(row[0].clone(), sql);
        }
    }

    Ok((schemas, table_map, view_map, macro_map))
}

#[pyfunction]
pub fn stage_host_temp_tables(target: &Bound<'_, PyAny>) -> (Vec<String>, Vec<(String, String)>) {
    let _ = target.call_method1("execute", ("ATTACH IF NOT EXISTS ':memory:' AS memory;",));

    let temp_tables: Vec<String> = target
        .call_method1("execute", ("SELECT table_name FROM duckdb_tables() WHERE temporary = true AND internal = false;",))
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String,)>>().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(t,)| t)
        .collect();

    for t in &temp_tables {
        let safe_t = t.replace('\"', "\"\"");
        let stage_sql = format!("CREATE OR REPLACE TABLE memory.\"__duckpack_temp_bridge_{}\" AS SELECT * FROM \"{}\";", safe_t, safe_t);
        let _ = target.call_method1("execute", (stage_sql,));
    }

    let temp_views: Vec<(String, String)> = target
        .call_method1("execute", ("SELECT view_name, sql FROM duckdb_views() WHERE temporary = true AND internal = false;",))
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String, String)>>().ok())
        .unwrap_or_default();

    let _ = target.call_method1("execute", ("CREATE TABLE IF NOT EXISTS memory.__duckpack_temp_views_registry (view_name VARCHAR, view_sql VARCHAR);",));
    if !temp_views.is_empty() {
        for (vname, vsql) in &temp_views {
            let safe_vname = vname.replace('\'', "''");
            let safe_vsql = vsql.replace('\'', "''");
            let ins_sql = format!("INSERT INTO memory.__duckpack_temp_views_registry VALUES ('{}', '{}');", safe_vname, safe_vsql);
            let _ = target.call_method1("execute", (ins_sql,));
        }
    }

    (temp_tables, temp_views)
}

#[pyfunction]
pub fn sync_back_host_temp_objects(target: &Bound<'_, PyAny>) {
    let _ = target.call_method1("execute", ("ATTACH IF NOT EXISTS ':memory:' AS memory;",));

    // 1. Restore tables from server default db
    let bridge_tables: Vec<String> = target
        .call_method1("execute", ("SELECT table_name FROM duckdb_tables() WHERE table_name LIKE '__duckpack_temp_bridge_%' AND internal = false;",))
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String,)>>().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(t,)| t)
        .collect();

    for bt in bridge_tables {
        if let Some(orig) = bt.strip_prefix("__duckpack_temp_bridge_") {
            let safe_orig = orig.replace('\"', "\"\"");
            let safe_bt = bt.replace('\"', "\"\"");
            let restore_sql = format!(
                "CREATE OR REPLACE TEMP TABLE \"{}\" AS SELECT * FROM \"{}\"; DROP TABLE IF EXISTS \"{}\";",
                safe_orig, safe_bt, safe_bt
            );
            let _ = target.call_method1("execute", (restore_sql,));
        }
    }

    // 2. Restore tables from memory
    let mem_bridges: Vec<String> = target
        .call_method1("execute", ("SELECT table_name FROM duckdb_tables() WHERE database_name = 'memory' AND table_name LIKE '__duckpack_temp_bridge_%' AND internal = false;",))
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String,)>>().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(t,)| t)
        .collect();

    for bt in mem_bridges {
        if let Some(orig) = bt.strip_prefix("__duckpack_temp_bridge_") {
            let safe_orig = orig.replace('\"', "\"\"");
            let safe_bt = bt.replace('\"', "\"\"");
            let restore_sql = format!(
                "CREATE OR REPLACE TEMP TABLE \"{}\" AS SELECT * FROM memory.\"{}\"; DROP TABLE IF EXISTS memory.\"{}\";",
                safe_orig, safe_bt, safe_bt
            );
            let _ = target.call_method1("execute", (restore_sql,));
        }
    }

    // 3. Restore views
    let views_to_sync: Vec<(String, String)> = target
        .call_method1("execute", ("SELECT view_name, view_sql FROM memory.__duckpack_temp_views_sync_back;",))
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String, String)>>().ok())
        .unwrap_or_default();

    for (_vname, vsql) in views_to_sync {
        let _ = target.call_method1("execute", (vsql,));
    }
    let _ = target.call_method1("execute", ("DROP TABLE IF EXISTS memory.__duckpack_temp_views_sync_back;",));
}

fn connection_has_temp_objects(target: &Bound<'_, PyAny>) -> bool {
    target
        .call_method1("execute", ("SELECT count(*) FROM duckdb_tables() WHERE temporary = true AND internal = false;",))
        .ok()
        .and_then(|res| res.call_method0("fetchone").ok())
        .and_then(|row| row.extract::<(i64,)>().ok())
        .map(|(c,)| c > 0)
        .unwrap_or(false)
        || target
            .call_method1("execute", ("SELECT count(*) FROM duckdb_views() WHERE temporary = true AND internal = false;",))
            .ok()
            .and_then(|res| res.call_method0("fetchone").ok())
            .and_then(|row| row.extract::<(i64,)>().ok())
            .map(|(c,)| c > 0)
            .unwrap_or(false)
}



/// Manages a local Quack network bridge for in-memory DuckDB connections or sessions with temporary objects.
/// Handles two-way table/view staging, port allocation, server startup, and clean teardown.
#[pyclass]
pub struct SessionBridge {
    target: Py<PyAny>,
    port: u16,
    db_uri: String,
    token: String,
    is_active: bool,
}

#[pymethods]
impl SessionBridge {
    #[new]
    #[pyo3(signature = (target, token = None, port = None))]
    pub fn new(
        target: Bound<'_, PyAny>,
        token: Option<&str>,
        port: Option<u16>,
    ) -> Self {
        let resolved_port = port.unwrap_or_else(find_available_port);
        let resolved_token = token.unwrap_or("duckpack_py").to_string();
        let db_uri = format!("quack:127.0.0.1:{}", resolved_port);
        Self {
            target: target.unbind(),
            port: resolved_port,
            db_uri,
            token: resolved_token,
            is_active: false,
        }
    }

    /// Stages temporary tables and views into memory, loads Quack, and begins serving.
    /// Returns the database URI (e.g. 'quack:127.0.0.1:9494').
    pub fn start(&mut self, py: Python<'_>) -> PyResult<String> {
        let target = self.target.bind(py);
        stage_host_temp_tables(target);

        let serve_sql = format!(
            "INSTALL quack; LOAD quack; CALL quack_serve('{}', token := '{}');",
            self.db_uri, self.token
        );
        if let Err(e) = target.call_method1("execute", (serve_sql,)) {
            return Err(ExecutionError::new_err(format!(
                "Unable to start local quack bridge server for in-memory connection: {}. \
                Tip: Ensure the 'quack' extension is installed and can be loaded in DuckDB.",
                e
            )));
        }

        self.is_active = true;
        Ok(self.db_uri.clone())
    }

    /// Syncs back any modified temporary tables and views, then stops the background Quack server.
    pub fn stop(&mut self, py: Python<'_>) -> PyResult<bool> {
        if !self.is_active {
            return Ok(false);
        }

        let target = self.target.bind(py);
        sync_back_host_temp_objects(target);

        let stop_sql = format!("CALL quack_stop('{}');", self.db_uri);
        let _ = target.call_method1("execute", (stop_sql,));

        self.is_active = false;
        Ok(true)
    }

    #[getter]
    pub fn db_uri(&self) -> String {
        self.db_uri.clone()
    }

    #[getter]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[getter]
    pub fn token(&self) -> String {
        self.token.clone()
    }

    #[getter]
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Returns True if the connection contains temporary tables or views that require bridging.
    #[staticmethod]
    pub fn has_temp_objects(target: &Bound<'_, PyAny>) -> bool {
        connection_has_temp_objects(target)
    }

    pub fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        Ok(slf)
    }

    pub fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: Option<&Bound<'_, PyAny>>,
        _exc_val: Option<&Bound<'_, PyAny>>,
        _exc_tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        self.stop(py)?;
        Ok(false)
    }
}


/// Launch the interactive DuckPack IDE terminal UI against a target database.
/// In non-terminal environments (such as Jupyter notebooks), prints a formatted database overview.
#[pyfunction]
#[pyo3(signature = (target = None, project_dir = ".", database = None, catalog = None, quack_token = None, insecure = false, env = None))]
pub fn explore(
    py: Python<'_>,
    target: Option<&Bound<'_, PyAny>>,
    project_dir: &str,
    database: Option<&str>,
    catalog: Option<&str>,
    quack_token: Option<&str>,
    insecure: bool,
    env: Option<&str>,
) -> PyResult<()> {
    let mut p_dir = PathBuf::from(project_dir);
    if p_dir == PathBuf::from(".") {
        let disc = engine::discover_project_dir(&p_dir);
        if disc != p_dir {
            p_dir = disc;
        }
    }

    let explicit_db = target.and_then(|t| t.extract::<String>().ok());
    let explicit_catalogs = database.or(catalog).map(|cat| vec![cat.to_string()]);
    let spec = engine::TargetResolver::resolve(
        &p_dir,
        env,
        explicit_db.as_deref(),
        quack_token,
        Some(insecure),
        explicit_catalogs,
    );
    let req_catalog = spec.catalogs.first().cloned();

    if target.is_none() || explicit_db.is_some() {
        if !is_interactive_terminal() {
            let conn = spec.connect()
                .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
            return print_non_terminal_db(&conn, &spec.db, req_catalog.as_deref());
        }

        let _guard = TerminalGuard::enter()
            .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

        let ide_res = py.allow_threads(|| -> anyhow::Result<()> {
            let conn = spec.connect()?;
            if let Some(cat) = &req_catalog {
                let _ = conn.execute_batch(&format!("USE \"{}\";", cat.replace('\"', "\"\"")));
            }

            if spec.is_quack {
                engine::cleanup_orphaned_quack_bridges(&conn);
            }

            crate::ide::run_ide_loop(&mut terminal, &conn, &p_dir, &spec.db, spec.disable_ssl)
        });

        ide_res.map_err(|e| ExecutionError::new_err(format!("IDE error: {}", e)))?;

        Ok(())
    } else {
        let target = target.unwrap();
        // Live Python connection
        let mut executor = PyDbExecutor::new(target.clone().unbind());

        if !is_interactive_terminal() {
            return print_non_terminal_explorer(&mut executor, req_catalog.as_deref());
        }

        let _guard = TerminalGuard::enter()
            .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

        if let Some(cat) = &req_catalog {
            let _ = target.call_method1("execute", (format!("USE \"{}\";", cat.replace('\"', "\"\"")),));
        }

        // Live DuckDB connection in Python:
        let db_name = target
            .getattr("execute")
            .ok()
            .and_then(|_| {
                target.call_method1("execute", ("PRAGMA database_list;",)).ok()
            })
            .and_then(|res| res.call_method0("fetchall").ok())
            .and_then(|rows| {
                if let Ok(r) = rows.extract::<Vec<(Bound<'_, PyAny>, Bound<'_, PyAny>, String)>>() {
                    r.into_iter().next().map(|t| t.2)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ":memory:".to_string());

        let mut active_bridge: Option<SessionBridge> = None;
        let has_temp = connection_has_temp_objects(target);

        let active_db_target = if !db_name.is_empty() && db_name != ":memory:" && !has_temp {
            match engine::connect_target_db(&db_name, None, false) {
                Ok(_) => db_name,
                Err(e) => {
                    // Fall back to serving via Python's active connection using SessionBridge
                    eprintln!("💡 Notice: Direct connection to target database file '{}' failed: {}.", db_name, e);
                    eprintln!("   Seamlessly falling back to live Python Quack bridge...");

                    let mut bridge = SessionBridge::new(target.clone(), Some("duckpack_py"), None);
                    let server_uri = bridge.start(py)?;
                    active_bridge = Some(bridge);
                    server_uri
                }
            }
        } else {
            // For in-memory connection, start quack_serve on an available port using SessionBridge
            let mut bridge = SessionBridge::new(target.clone(), Some("duckpack_py"), None);
            let server_uri = bridge.start(py)?;
            active_bridge = Some(bridge);
            server_uri
        };

        let is_bridge = active_bridge.is_some();
        let ide_res = py.allow_threads(|| -> anyhow::Result<()> {
            let conn = if is_bridge {
                engine::connect_target_db(&active_db_target, Some("duckpack_py"), false)?
            } else {
                engine::connect_target_db(&active_db_target, None, false)?
            };

            if is_bridge {
                engine::restore_quack_temp_bridge(&conn);
            }

            if let Some(cat) = &req_catalog {
                let _ = conn.execute_batch(&format!("USE \"{}\";", cat.replace('\"', "\"\"")));
            }

            let loop_res = crate::ide::run_ide_loop(&mut terminal, &conn, &p_dir, &active_db_target, is_bridge);

            if is_bridge {
                engine::sync_back_quack_temp_bridge(&conn);
            }

            loop_res
        });

        if let Some(mut bridge) = active_bridge {
            let _ = bridge.stop(py);
        }
        ide_res.map_err(|e| ExecutionError::new_err(format!("IDE error: {}", e)))?;

        Ok(())
    }
}

/// Helper function to inspect the resolved target configuration from duckpack.toml and .env.
#[pyfunction]
#[pyo3(signature = (project_dir = ".", env = None))]
pub fn get_target_config(
    py: Python<'_>,
    project_dir: &str,
    env: Option<&str>,
) -> PyResult<PyObject> {
    let mut p_dir = PathBuf::from(project_dir);
    if p_dir == PathBuf::from(".") {
        let disc = engine::discover_project_dir(&p_dir);
        if disc != p_dir {
            p_dir = disc;
        }
    }

    engine::load_env_files(&p_dir, env);

    let cfg = engine::resolve_target_config(&p_dir, env);
    let token = engine::resolve_quack_token(None, cfg.token_env.as_deref());

    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("db", cfg.db)?;
    dict.set_item("catalogs", cfg.catalogs)?;
    dict.set_item("token_env", cfg.token_env)?;
    dict.set_item("insecure", cfg.insecure.unwrap_or(false))?;
    dict.set_item("token", token)?;
    Ok(dict.into())
}

/// Python module definition
#[pymodule]
pub fn _duckpack(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("Error", m.py().get_type::<Error>())?;
    m.add("SchemaError", m.py().get_type::<SchemaError>())?;
    m.add("ExecutionError", m.py().get_type::<ExecutionError>())?;
    m.add_class::<ApplyResult>()?;
    m.add_class::<DiffSummary>()?;
    m.add_class::<SessionBridge>()?;
    m.add_function(wrap_pyfunction!(explore, m)?)?;
    m.add_function(wrap_pyfunction!(apply, m)?)?;
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(build, m)?)?;
    m.add_function(wrap_pyfunction!(init, m)?)?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(get_target_config, m)?)?;
    m.add_function(wrap_pyfunction!(stage_host_temp_tables, m)?)?;
    m.add_function(wrap_pyfunction!(sync_back_host_temp_objects, m)?)?;
    m.add("__duckdb_version__", crate::engine::embedded_duckdb_version())?;
    Ok(())
}
