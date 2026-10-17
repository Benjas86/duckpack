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

fn print_non_terminal_explorer(executor: &mut PyDbExecutor) -> PyResult<()> {
    let (_schemas, table_map, view_map, macro_map) = introspect_py_catalog(executor)?;

    println!("======================================================================");
    println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode)");
    println!("======================================================================");

    if table_map.is_empty() {
        println!("📊 Tables: (none)");
    } else {
        println!("📊 Tables ({}):", table_map.len());
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
    }

    if !view_map.is_empty() {
        println!("\n🔍 Views ({}):", view_map.len());
        for name in view_map.keys() {
            println!("  • {}", name);
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

fn print_non_terminal_db(conn: &duckdb::Connection, db_name: &str) -> PyResult<()> {
    let tables = schema_diff::get_schema(conn).unwrap_or_default();
    let views = schema_diff::get_views(conn).unwrap_or_default();
    let macros = schema_diff::get_macros(conn).unwrap_or_default();

    println!("======================================================================");
    println!("🦆 DuckPack Database Explorer (Notebook / Non-TTY Mode: {})", db_name);
    println!("======================================================================");

    if tables.is_empty() {
        println!("📊 Tables: (none)");
    } else {
        println!("📊 Tables ({}):", tables.len());
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
    }

    if !views.is_empty() {
        println!("\n🔍 Views ({}):", views.len());
        for name in views.keys() {
            println!("  • {}", name);
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
#[pyo3(signature = (target, project_dir = "."))]
pub fn diff(_py: Python<'_>, target: &Bound<'_, PyAny>, project_dir: &str) -> PyResult<DiffSummary> {
    let p_dir = PathBuf::from(project_dir);
    let shadow_conn = engine::build_shadow_db(&p_dir)
        .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

    let shadow_schemas = schema_diff::get_schemas(&shadow_conn)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow schemas: {}", e)))?;
    let shadow_schema = schema_diff::get_schema(&shadow_conn)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow schema: {}", e)))?;
    let shadow_views = schema_diff::get_views(&shadow_conn)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow views: {}", e)))?;
    let shadow_macros = schema_diff::get_macros(&shadow_conn)
        .map_err(|e| SchemaError::new_err(format!("Error extracting shadow macros: {}", e)))?;

    // Determine target connection
    let (target_schemas, target_schema, target_views, target_macros) = if let Ok(db_str) = target.extract::<String>() {
        let conn = engine::connect_target_db(&db_str, None, false)
            .map_err(|e| ExecutionError::new_err(format!("Failed to connect to {}: {}", db_str, e)))?;
        (
            schema_diff::get_schemas(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_schema(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_views(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?,
            schema_diff::get_macros(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?,
        )
    } else {
        // Live Python connection: introspect catalog directly
        let mut executor = PyDbExecutor::new(target.clone().unbind());
        introspect_py_catalog(&mut executor)?
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

/// Apply declarative schema migrations to the target database.
#[pyfunction]
#[pyo3(signature = (target, project_dir = ".", _auto_approve = true, force_drop = false, _interactive = false))]
pub fn apply(
    _py: Python<'_>,
    target: &Bound<'_, PyAny>,
    project_dir: &str,
    _auto_approve: bool,
    force_drop: bool,
    _interactive: bool,
) -> PyResult<ApplyResult> {
    let p_dir = PathBuf::from(project_dir);
    if !p_dir.exists() {
        return Err(SchemaError::new_err(format!("Project directory {} does not exist", p_dir.display())));
    }

    let ignore_list = engine::load_ignore_rules(&p_dir);

    if let Ok(db_str) = target.extract::<String>() {
        let conn = engine::connect_target_db(&db_str, None, false)
            .map_err(|e| ExecutionError::new_err(format!("Failed to connect to {}: {}", db_str, e)))?;

        let shadow_conn = engine::build_shadow_db(&p_dir)
            .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

        let shadow_schemas = schema_diff::get_schemas(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_schemas = schema_diff::get_schemas(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_schema = schema_diff::get_schema(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_schema = schema_diff::get_schema(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_views = schema_diff::get_views(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_views = schema_diff::get_views(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?;
        let shadow_macros = schema_diff::get_macros(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let target_macros = schema_diff::get_macros(&conn).map_err(|e| ExecutionError::new_err(e.to_string()))?;

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

        let remote = engine::is_remote(&db_str);
        engine::apply_diff(&conn, &diff_result, force_drop, remote, &p_dir)
            .map_err(|e| ExecutionError::new_err(format!("Apply error: {}", e)))?;

        let applied_items: Vec<String> = diff_result
            .items
            .iter()
            .filter(|i| i.selected && !i.ignored)
            .map(|i| i.obj_name.clone())
            .collect();
        let applied_count = applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: applied_items,
        })
    } else {
        // Live Python connection
        let mut executor = PyDbExecutor::new(target.clone().unbind());
        let shadow_conn = engine::build_shadow_db(&p_dir)
            .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

        let shadow_schemas = schema_diff::get_schemas(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let shadow_schema = schema_diff::get_schema(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let shadow_views = schema_diff::get_views(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;
        let shadow_macros = schema_diff::get_macros(&shadow_conn).map_err(|e| SchemaError::new_err(e.to_string()))?;

        let (target_schemas, target_schema, target_views, target_macros) = introspect_py_catalog(&mut executor)?;

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

        let applied_items: Vec<String> = diff_result
            .items
            .iter()
            .filter(|i| i.selected && !i.ignored)
            .map(|i| i.obj_name.clone())
            .collect();
        let applied_count = applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: applied_items,
        })
    }
}

/// Helper to introspect tables, views, and macros from a PyDbExecutor
fn introspect_py_catalog(
    executor: &mut PyDbExecutor,
) -> PyResult<(
    HashSet<String>,
    std::collections::HashMap<String, schema_diff::TableSchema>,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
)> {
    use std::collections::HashMap;

    // 1. Schemas
    let (_, schema_rows) = executor
        .query_rows("SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('information_schema', 'pg_catalog', 'main')")
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let schemas: HashSet<String> = schema_rows.into_iter().filter_map(|r| r.into_iter().next()).collect();

    // 2. Tables
    let (_, table_rows) = executor
        .query_rows("SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, COALESCE(sql, '') FROM duckdb_tables() WHERE internal = false")
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
    let (_, col_rows) = executor
        .query_rows("SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, column_name, data_type FROM duckdb_columns() WHERE internal = false")
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    for row in col_rows {
        if row.len() >= 3 {
            if let Some(t) = table_map.get_mut(&row[0]) {
                t.columns.insert(row[1].clone(), row[2].clone());
            }
        }
    }

    // Check for attached quack databases
    if let Ok((_, db_rows)) = executor.query_rows("SELECT database_name, type FROM duckdb_databases() WHERE internal = false") {
        for db in db_rows {
            if db.len() >= 2 && db[1].eq_ignore_ascii_case("quack") {
                let db_name = &db[0];
                let safe_name = db_name.replace("'", "''");
                let tbl_sql = format!(
                    "SELECT '{}.' || (CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END), COALESCE(sql, '') FROM {}.query('SELECT schema_name, table_name, sql FROM duckdb_tables() WHERE internal = false')",
                    safe_name, safe_name
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
                    safe_name, safe_name
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
        }
    }

    // 4. Views
    let (_, view_rows) = executor
        .query_rows("SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, COALESCE(sql, '') FROM duckdb_views() WHERE internal = false")
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    let mut view_map = HashMap::new();
    for row in view_rows {
        if row.len() >= 2 {
            view_map.insert(row[0].clone(), row[1].clone());
        }
    }

    // 5. Macros
    let (_, macro_rows) = executor
        .query_rows("SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END, macro_definition, CAST(parameters AS VARCHAR) FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false")
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

/// Launch the interactive DuckPack IDE terminal UI against a target database.
/// In non-terminal environments (such as Jupyter notebooks), prints a formatted database overview.
#[pyfunction]
#[pyo3(signature = (target, project_dir = ".", quack_token = None, insecure = false))]
pub fn explore(
    _py: Python<'_>,
    target: &Bound<'_, PyAny>,
    project_dir: &str,
    quack_token: Option<&str>,
    insecure: bool,
) -> PyResult<()> {
    let p_dir = PathBuf::from(project_dir);

    if let Ok(db_str) = target.extract::<String>() {
        let conn = engine::connect_target_db(&db_str, quack_token, insecure)
            .map_err(|e| ExecutionError::new_err(format!("Failed to connect to {}: {}", db_str, e)))?;

        if !is_interactive_terminal() {
            return print_non_terminal_db(&conn, &db_str);
        }

        let _guard = TerminalGuard::enter()
            .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

        crate::ide::run_ide_loop(&mut terminal, &conn, &p_dir, &db_str)
            .map_err(|e| ExecutionError::new_err(format!("IDE error: {}", e)))?;

        Ok(())
    } else {
        // Live Python connection
        let mut executor = PyDbExecutor::new(target.clone().unbind());

        if !is_interactive_terminal() {
            return print_non_terminal_explorer(&mut executor);
        }

        let _guard = TerminalGuard::enter()
            .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

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

        let (conn, active_db_target) = if !db_name.is_empty() && db_name != ":memory:" {
            let c = engine::connect_target_db(&db_name, None, false)
                .map_err(|e| ExecutionError::new_err(format!("Failed to connect to {}: {}", db_name, e)))?;
            (c, db_name)
        } else {
            // For in-memory connection, start quack_serve on an available port
            let port = find_available_port();
            let start_server_sql = format!(
                "INSTALL quack; LOAD quack; CALL quack_serve('quack:localhost:{}', token := 'duckpack_py');",
                port
            );
            if let Err(e) = target.call_method1("execute", (start_server_sql,)) {
                return Err(ExecutionError::new_err(format!(
                    "Unable to start local quack bridge for in-memory DuckDB connection: {}. \
                    Tip: Ensure the 'quack' extension can be loaded, or explore a persistent database file via duckpack.explore('path.duckdb').",
                    e
                )));
            }

            let server_uri = format!("quack:localhost:{}", port);
            let c = engine::connect_target_db(&server_uri, Some("duckpack_py"), false)
                .map_err(|e| ExecutionError::new_err(format!("Failed to attach to Python in-memory DuckDB: {}", e)))?;
            (c, server_uri)
        };

        let ide_res = crate::ide::run_ide_loop(&mut terminal, &conn, &p_dir, &active_db_target);
        if active_db_target.starts_with("quack:") {
            let _ = target.call_method1("execute", (format!("CALL quack_stop('{}');", active_db_target),));
        }
        ide_res.map_err(|e| ExecutionError::new_err(format!("IDE error: {}", e)))?;

        Ok(())
    }
}

/// Python module definition
#[pymodule]
pub fn _duckpack(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("Error", m.py().get_type::<Error>())?;
    m.add("SchemaError", m.py().get_type::<SchemaError>())?;
    m.add("ExecutionError", m.py().get_type::<ExecutionError>())?;
    m.add_class::<ApplyResult>()?;
    m.add_class::<DiffSummary>()?;
    m.add_function(wrap_pyfunction!(explore, m)?)?;
    m.add_function(wrap_pyfunction!(apply, m)?)?;
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(build, m)?)?;
    m.add_function(wrap_pyfunction!(init, m)?)?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    Ok(())
}
