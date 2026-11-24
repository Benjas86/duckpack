use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::diff as schema_diff;
use crate::engine::{self, DbExecutor};
use crate::session::{
    apply_catalogs_with_executor, format_non_terminal_summary, open_explore_session, TerminalGuard,
};

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
                                row_data.push(
                                    cell.str()
                                        .map(|s| s.to_string())
                                        .unwrap_or_else(|_| format!("{:?}", cell)),
                                );
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

fn is_interactive_terminal() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9494)
}

fn print_non_terminal_summary_exec(
    exec: &mut dyn DbExecutor,
    db_label: Option<&str>,
    catalog: Option<&str>,
) -> PyResult<()> {
    let summary = format_non_terminal_summary(exec, db_label, catalog)
        .map_err(|e| ExecutionError::new_err(e.to_string()))?;
    println!("{}", summary);
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
        Self {
            success,
            applied_count,
            items,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<ApplyResult success={} applied_count={}>",
            self.success, self.applied_count
        )
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
        Self {
            has_changes,
            change_count,
            items,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<DiffSummary has_changes={} change_count={}>",
            self.has_changes, self.change_count
        )
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
    let mut out_conn = duckdb::Connection::open(out)
        .map_err(|e| ExecutionError::new_err(format!("Failed to create .duckpack: {}", e)))?;

    engine::compile_project_declarations(&mut out_conn, p_dir)
        .map_err(|e| SchemaError::new_err(format!("Compilation error: {}", e)))?;

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
    let (_shadow_conn, shadow_snapshot) = schema_diff::CatalogSnapshot::from_project_dir(&p_dir)
        .map_err(|e| SchemaError::new_err(format!("Shadow DB failed: {}", e)))?;

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

    let target_snapshot = if target.is_none() || explicit_db.is_some() {
        let mut conn = spec
            .connect()
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
        let _ = crate::workspace::apply_workspace_secrets(&conn, &p_dir, env)
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
        let active_cat =
            schema_diff::validate_target_catalog_exec(&mut conn, req_catalog.as_deref(), false)
                .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        schema_diff::CatalogSnapshot::introspect(&mut conn, Some(&active_cat))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?
    } else {
        let t = target.unwrap();
        let mut executor = PyDbExecutor::new(t.clone().unbind());
        let active_cat =
            schema_diff::validate_target_catalog_exec(&mut executor, req_catalog.as_deref(), false)
                .map_err(|e| ExecutionError::new_err(e.to_string()))?;
        schema_diff::CatalogSnapshot::introspect(&mut executor, Some(&active_cat))
            .map_err(|e| ExecutionError::new_err(e.to_string()))?
    };

    let ignore_list = engine::load_ignore_rules(&p_dir);
    let diff_result = schema_diff::compute_diff_snapshots(
        &shadow_snapshot,
        &target_snapshot,
        &p_dir,
        &ignore_list,
    );

    let unignored_items: Vec<_> = diff_result
        .items
        .into_iter()
        .filter(|i| !i.ignored)
        .collect();
    let change_count = unignored_items.len();
    let items = unignored_items
        .iter()
        .map(|item| format!("{:?}: {}", item.item_type, item.obj_name))
        .collect();

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
    allow_lake_rewrite: bool,
    catalog: Option<&str>,
) -> PyResult<ApplyResult> {
    let _guard = TerminalGuard::enter()
        .map_err(|e| ExecutionError::new_err(format!("Failed to initialize terminal: {}", e)))?;

    let stdout = std::io::stdout();
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)
        .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

    let summary = crate::tui::run_diff_review_session_with_options(
        &mut terminal,
        conn,
        project_dir,
        db_target,
        catalog,
        force_drop,
        allow_lake_rewrite,
        false,
    )
    .map_err(|e| ExecutionError::new_err(format!("TUI error: {}", e)))?;

    Ok(ApplyResult {
        success: summary.total_applied_count > 0,
        applied_count: summary.total_applied_count,
        items: summary.applied_items,
    })
}

/// Apply declarative schema migrations to the target database.
#[pyfunction]
#[pyo3(signature = (target = None, project_dir = ".", database = None, catalog = None, auto_approve = true, force_drop = false, allow_lake_rewrite = false, interactive = false, quack_token = None, insecure = false, env = None))]
pub fn apply(
    py: Python<'_>,
    target: Option<&Bound<'_, PyAny>>,
    project_dir: &str,
    database: Option<&str>,
    catalog: Option<&str>,
    auto_approve: bool,
    force_drop: bool,
    allow_lake_rewrite: bool,
    interactive: bool,
    quack_token: Option<&str>,
    insecure: bool,
    env: Option<&str>,
) -> PyResult<ApplyResult> {
    let p_dir = PathBuf::from(project_dir);
    if !p_dir.exists() {
        return Err(SchemaError::new_err(format!(
            "Project directory {} does not exist",
            p_dir.display()
        )));
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

    if target.is_none() || explicit_db.is_some() {
        let mut conn = spec
            .connect()
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
        let _ = crate::workspace::apply_workspace_secrets(&conn, &p_dir, env)
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;

        if (!auto_approve || interactive) && is_interactive_terminal() {
            let start_cat = req_catalogs.first().map(|s| s.as_str());
            return run_apply_tui(
                &conn,
                &p_dir,
                &spec.db,
                force_drop,
                allow_lake_rewrite,
                start_cat,
            );
        } else if !auto_approve {
            return Err(ExecutionError::new_err(
                "Cannot apply schema changes: auto_approve is False in a non-interactive environment. \
                 Pass auto_approve=True to approve and apply migrations automatically.",
            ));
        }

        let report = apply_catalogs_with_executor(
            &mut conn,
            &p_dir,
            &req_catalogs,
            force_drop,
            allow_lake_rewrite,
            engine::is_remote(&spec.db),
            false,
        )
        .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;

        let applied_count = report.applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: report.applied_items,
        })
    } else {
        let target = target.unwrap();
        if (!auto_approve || interactive) && is_interactive_terminal() {
            let db_name = target
                .getattr("execute")
                .ok()
                .and_then(|_| {
                    target
                        .call_method1("execute", ("PRAGMA database_list;",))
                        .ok()
                })
                .and_then(|res| res.call_method0("fetchall").ok())
                .and_then(|rows| {
                    if let Ok(r) =
                        rows.extract::<Vec<(Bound<'_, PyAny>, Bound<'_, PyAny>, String)>>()
                    {
                        r.into_iter().next().map(|t| t.2)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| ":memory:".to_string());

            let direct_conn = if !db_name.is_empty() && db_name != ":memory:" {
                engine::connect_target_db(&db_name, None, false).ok()
            } else {
                None
            };

            let start_cat = req_catalogs.first().map(|s| s.as_str());
            if let Some(conn) = direct_conn {
                return run_apply_tui(
                    &conn,
                    &p_dir,
                    &db_name,
                    force_drop,
                    allow_lake_rewrite,
                    start_cat,
                );
            }

            let mut bridge = SessionBridge::new(target.clone(), None, None);
            let bridge_token = bridge.token();
            let active_db_target = bridge.start(py)?;
            let conn = engine::connect_target_db(&active_db_target, Some(&bridge_token), true)
                .map_err(|e| {
                    let _ = bridge.stop(py);
                    ExecutionError::new_err(format!(
                        "Failed to attach to Python Quack bridge: {}",
                        e
                    ))
                })?;

            engine::restore_quack_temp_bridge(&conn);
            let res = run_apply_tui(
                &conn,
                &p_dir,
                &active_db_target,
                force_drop,
                allow_lake_rewrite,
                start_cat,
            );
            engine::sync_back_quack_temp_bridge(&conn);
            drop(conn);
            let _ = bridge.stop(py);
            return res;
        } else if !auto_approve {
            return Err(ExecutionError::new_err(
                "Cannot apply schema changes: auto_approve is False in a non-interactive environment. \
                 Pass auto_approve=True to approve and apply migrations automatically.",
            ));
        }

        let mut executor = PyDbExecutor::new(target.clone().unbind());
        let report = apply_catalogs_with_executor(
            &mut executor,
            &p_dir,
            &req_catalogs,
            force_drop,
            allow_lake_rewrite,
            false,
            false,
        )
        .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;

        let applied_count = report.applied_items.len();
        Ok(ApplyResult {
            success: true,
            applied_count,
            items: report.applied_items,
        })
    }
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
        let stage_sql = format!(
            "CREATE OR REPLACE TABLE memory.\"__duckpack_temp_bridge_{}\" AS SELECT * FROM \"{}\";",
            safe_t, safe_t
        );
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
            let ins_sql = format!(
                "INSERT INTO memory.__duckpack_temp_views_registry VALUES ('{}', '{}');",
                safe_vname, safe_vsql
            );
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
        .call_method1(
            "execute",
            ("SELECT view_name, view_sql FROM memory.__duckpack_temp_views_sync_back;",),
        )
        .ok()
        .and_then(|res| res.call_method0("fetchall").ok())
        .and_then(|rows| rows.extract::<Vec<(String, String)>>().ok())
        .unwrap_or_default();

    for (_vname, vsql) in views_to_sync {
        if crate::sql_utils::validate_safe_temp_view_ddl(&vsql).is_ok() {
            let _ = target.call_method1("execute", (vsql,));
        }
    }
    let _ = target.call_method1(
        "execute",
        ("DROP TABLE IF EXISTS memory.__duckpack_temp_views_sync_back;",),
    );
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
    pub fn new(target: Bound<'_, PyAny>, token: Option<&str>, port: Option<u16>) -> Self {
        let resolved_port = port.unwrap_or_else(find_available_port);
        let resolved_token = token
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(engine::generate_random_hex_token);
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

        let safe_uri = self.db_uri.replace('\'', "''");
        let safe_token = self.token.replace('\'', "''");
        let serve_sql = format!(
            "INSTALL quack; LOAD quack; CALL quack_serve('{}', token := '{}');",
            safe_uri, safe_token
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

        let safe_uri = self.db_uri.replace('\'', "''");
        let stop_sql = format!("CALL quack_stop('{}');", safe_uri);
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
            let mut opened = open_explore_session(
                &p_dir,
                env,
                &spec,
                explicit_db.as_deref(),
                req_catalog.as_deref(),
                false,
            )
            .map_err(|e| ExecutionError::new_err(format!("{:#}", e)))?;
            return print_non_terminal_summary_exec(
                &mut opened.conn,
                Some(&opened.display_db),
                opened.active_catalog.as_deref(),
            );
        }

        let _guard = TerminalGuard::enter().map_err(|e| {
            ExecutionError::new_err(format!("Failed to initialize terminal: {}", e))
        })?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

        let ide_res = py.allow_threads(|| -> anyhow::Result<()> {
            let opened = open_explore_session(
                &p_dir,
                env,
                &spec,
                explicit_db.as_deref(),
                req_catalog.as_deref(),
                true,
            )?;

            crate::ide::run_ide_loop_with_workspace(
                &mut terminal,
                &opened.conn,
                &p_dir,
                &opened.display_db,
                spec.disable_ssl,
                env,
                opened.ws_report,
            )
        });

        ide_res.map_err(|e| ExecutionError::new_err(format!("IDE error: {}", e)))?;

        Ok(())
    } else {
        let target = target.unwrap();
        // Live Python connection
        let mut executor = PyDbExecutor::new(target.clone().unbind());

        if !is_interactive_terminal() {
            return print_non_terminal_summary_exec(&mut executor, None, req_catalog.as_deref());
        }

        let _guard = TerminalGuard::enter().map_err(|e| {
            ExecutionError::new_err(format!("Failed to initialize terminal: {}", e))
        })?;

        let stdout = std::io::stdout();
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|e| ExecutionError::new_err(format!("Terminal backend error: {}", e)))?;

        if let Some(cat) = &req_catalog {
            let _ = target.call_method1(
                "execute",
                (format!("USE \"{}\";", cat.replace('\"', "\"\"")),),
            );
        }

        let mut bridge = SessionBridge::new(target.clone(), None, None);
        let bridge_token = bridge.token();
        let active_db_target = bridge.start(py)?;
        let active_bridge: Option<SessionBridge> = Some(bridge);

        let is_bridge = true;
        let ide_res = py.allow_threads(|| -> anyhow::Result<()> {
            let conn = engine::connect_target_db(&active_db_target, Some(&bridge_token), true)?;

            if is_bridge {
                engine::restore_quack_temp_bridge(&conn);
            }

            let ws_report = crate::workspace::apply_workspace_attachments(&conn, &p_dir, env).ok();

            if let Some(cat) = &req_catalog {
                let _ = conn.execute_batch(&format!("USE \"{}\";", cat.replace('\"', "\"\"")));
            }

            let loop_res = crate::ide::run_ide_loop_with_workspace(
                &mut terminal,
                &conn,
                &p_dir,
                &active_db_target,
                is_bridge,
                env,
                ws_report,
            );

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
    let workspace_mode = crate::workspace::should_use_workspace_mode(&p_dir, env, None);

    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("db", cfg.db)?;
    dict.set_item("catalogs", cfg.catalogs)?;
    dict.set_item("token_env", cfg.token_env)?;
    dict.set_item("insecure", cfg.insecure.unwrap_or(false))?;
    dict.set_item("token", token)?;
    dict.set_item("workspace_mode", workspace_mode)?;
    Ok(dict.into())
}

/// Auto-discovers the nearest declarative DuckPack project directory starting from `start`.
#[pyfunction]
#[pyo3(signature = (start = "."))]
pub fn discover_project_dir(start: &str) -> String {
    engine::discover_project_dir(Path::new(start))
        .to_string_lossy()
        .to_string()
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
    m.add_function(wrap_pyfunction!(discover_project_dir, m)?)?;
    m.add_function(wrap_pyfunction!(stage_host_temp_tables, m)?)?;
    m.add_function(wrap_pyfunction!(sync_back_host_temp_objects, m)?)?;
    m.add("__version__", crate::engine::duckpack_version())?;
    m.add(
        "__duckdb_version__",
        crate::engine::embedded_duckdb_version(),
    )?;
    Ok(())
}
