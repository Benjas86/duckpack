use anyhow::Result;
use duckdb::Connection;
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Reads a SQL file and dynamically interpolates any `${VARIABLE_NAME}` templates
/// with the system's current environment variables (loaded from .env).
/// This is used for multi-tenant or multi-environment deployments.
/// Reads a SQL file from disk and replaces any `${VAR}` templates with environment variables.
/// Uses `OnceLock` to safely cache the compiled regex for performance.
pub fn read_sql_with_env<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let content = std::fs::read_to_string(path)?;

    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\$\{([A-Za-z0-9_]+)\}").unwrap());

    let replaced = re.replace_all(&content, |caps: &regex::Captures| {
        let var_name = &caps[1];
        std::env::var(var_name).unwrap_or_else(|_| caps[0].to_string())
    });

    Ok(replaced.into_owned())
}

/// Represents a constraint on a table (PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintSchema {
    pub constraint_type: String, // "PRIMARY KEY", "UNIQUE", "FOREIGN KEY", "CHECK"
    pub constraint_name: Option<String>,
    pub columns: Vec<String>,
    pub expression: Option<String>,
    pub referenced_table: Option<String>,
    pub referenced_columns: Vec<String>,
}

impl ConstraintSchema {
    pub fn matches(&self, other: &Self) -> bool {
        if !self
            .constraint_type
            .eq_ignore_ascii_case(&other.constraint_type)
        {
            return false;
        }
        let self_cols: Vec<String> = self.columns.iter().map(|c| c.to_lowercase()).collect();
        let other_cols: Vec<String> = other.columns.iter().map(|c| c.to_lowercase()).collect();
        if self_cols != other_cols {
            return false;
        }
        match self.constraint_type.to_uppercase().as_str() {
            "CHECK" => {
                let self_expr = self
                    .expression
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(|c| c == '(' || c == ')')
                    .trim();
                let other_expr = other
                    .expression
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(|c| c == '(' || c == ')')
                    .trim();
                self_expr.eq_ignore_ascii_case(other_expr)
            }
            "FOREIGN KEY" => {
                let self_tbl = self
                    .referenced_table
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase();
                let other_tbl = other
                    .referenced_table
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase();
                let self_simple = self_tbl.split('.').last().unwrap_or(&self_tbl);
                let other_simple = other_tbl.split('.').last().unwrap_or(&other_tbl);
                if self_simple != other_simple {
                    return false;
                }
                let self_ref_cols: Vec<String> = self
                    .referenced_columns
                    .iter()
                    .map(|c| c.to_lowercase())
                    .collect();
                let other_ref_cols: Vec<String> = other
                    .referenced_columns
                    .iter()
                    .map(|c| c.to_lowercase())
                    .collect();
                self_ref_cols == other_ref_cols
            }
            _ => true,
        }
    }
}

/// Represents the resolved structure of a DuckDB table.
/// Contains the raw creation SQL, a map of column names to their data types, and parsed constraints.
#[derive(Debug, Clone, Default)]
pub struct TableSchema {
    pub create_sql: String,
    pub columns: HashMap<String, String>,
    pub column_defaults: HashMap<String, String>,
    pub constraints: Vec<ConstraintSchema>,
}

/// Enumerates all the possible schema modifications that can occur.
/// Used to dictate how the apply engine behaves and renders them in the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffItemType {
    CreateSchema,
    DropSchema,
    CreateTable,
    AlterTable,
    DropTable,
    RenameTable,
    DropColumn,
    RenameColumn,
    AddConstraint,
    RecreateTable,
    CreateView,
    DropView,
    CreateMacro,
    DropMacro,
    CreateTrigger,
    DropTrigger,
}

impl DiffItemType {
    /// Returns true if this migration item drops an object or recreates a table via copy-and-swap.
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            DiffItemType::DropSchema
                | DiffItemType::DropTable
                | DiffItemType::DropColumn
                | DiffItemType::DropView
                | DiffItemType::DropMacro
                | DiffItemType::DropTrigger
                | DiffItemType::RecreateTable
        )
    }
}

/// Represents an individual trigger in a DuckDB catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSchema {
    pub schema_name: String,
    pub trigger_name: String,
    pub table_name: String,
    pub sql: String,
}

/// Point-in-time record of all declarative schema objects in a single database catalog.
#[derive(Debug, Clone, Default)]
pub struct CatalogSnapshot {
    pub schemas: std::collections::HashSet<String>,
    pub tables: HashMap<String, TableSchema>,
    pub views: HashMap<String, String>,
    pub macros: HashMap<String, String>,
    pub triggers: HashMap<String, TriggerSchema>,
}

/// Validates that an object or asset name is a safe flat filename without path traversal components.
pub fn validate_safe_project_filename(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Object name cannot be empty");
    }
    if trimmed == "."
        || trimmed == ".."
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.chars().any(|c| c.is_control())
    {
        anyhow::bail!(
            "Unsafe object name '{}': path separators, '..', and control characters are not allowed",
            name
        );
    }
    Ok(())
}

impl CatalogSnapshot {
    /// Introspects all declarative schema objects (schemas, tables, views, macros, triggers)
    /// from any `DbExecutor` adapter for the specified catalog.
    pub fn introspect(
        exec: &mut dyn crate::engine::DbExecutor,
        catalog: Option<&str>,
    ) -> Result<Self> {
        Ok(Self {
            schemas: get_schemas_exec(exec, catalog)?,
            tables: get_schema_exec(exec, catalog)?,
            views: get_views_exec(exec, catalog)?,
            macros: get_macros_exec(exec, catalog)?,
            triggers: get_triggers_exec(exec, catalog)?,
        })
    }

    /// Compiles a declarative project directory into an in-memory Shadow Database and returns
    /// both the connection and its extracted `CatalogSnapshot`.
    pub fn from_project_dir(project_dir: &Path) -> Result<(Connection, Self)> {
        let mut shadow_conn = crate::engine::build_shadow_db(project_dir)?;
        let snapshot = Self::introspect(&mut shadow_conn, None)?;
        Ok((shadow_conn, snapshot))
    }

    /// Writes a reverse-engineered SQL definition from this `CatalogSnapshot` into the project directory.
    pub fn pull_object_to_project(
        &self,
        project_dir: &Path,
        item_type: &DiffItemType,
        obj_name: &str,
    ) -> Result<std::path::PathBuf> {
        validate_safe_project_filename(obj_name)?;
        let (subdir, sql) = match item_type {
            DiffItemType::DropTable => (
                "tables",
                self.tables.get(obj_name).map(|t| t.create_sql.clone()),
            ),
            DiffItemType::DropView => ("views", self.views.get(obj_name).cloned()),
            DiffItemType::DropMacro => ("macros", self.macros.get(obj_name).cloned()),
            DiffItemType::DropTrigger => (
                "triggers",
                self.triggers.get(obj_name).map(|t| t.sql.clone()),
            ),
            _ => anyhow::bail!("Unsupported item type for pull: {:?}", item_type),
        };
        let sql = sql.ok_or_else(|| {
            anyhow::anyhow!("Object '{}' not found in target catalog snapshot", obj_name)
        })?;
        let dir = project_dir.join(subdir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.sql", obj_name));
        std::fs::write(&path, sql)?;
        Ok(path)
    }
}

/// A discrete change to the database schema computed by the engine.
/// Holds the before/after state, the generated SQL to execute, and UI state (selected, locked).
#[derive(Debug, Clone)]
pub struct DiffItem {
    pub item_type: DiffItemType,
    pub obj_name: String,
    pub sql: String,
    pub old_def: Option<String>,
    pub new_def: Option<String>,
    pub selected: bool,
    pub locked: bool,
    pub ignored: bool,
}

impl DiffItem {
    pub fn is_destructive(&self) -> bool {
        self.item_type.is_destructive()
    }
}

/// The final collection of computed schema differences between the target and shadow databases.
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub items: Vec<DiffItem>,
}

impl DiffResult {
    pub fn is_empty(&self) -> bool {
        !self.items.iter().any(|i| !i.ignored)
    }
}

/// Extracts the entire table schema from a DuckDB connection.
/// It queries `sqlite_master` for table definitions and `duckdb_columns()` for specific datatypes.
/// Queries the DuckDB database to extract the structural schema of all tables.
///
/// # Educational Note:
/// We query `duckdb_tables()` to get the raw `CREATE TABLE` SQL,
/// and `duckdb_columns()` to map column names to data types.
/// Returns a HashMap mapping `schema.table` -> `TableSchema`.
#[derive(Debug, Clone)]
pub struct AttachedDatabase {
    pub name: String,
    pub db_type: String,
    pub path: Option<String>,
    pub readonly: bool,
}

fn is_target_db_quack(exec: &mut dyn crate::engine::DbExecutor) -> bool {
    crate::engine::is_quack_target_attached(exec)
}

fn detect_quack_alias(exec: &mut dyn crate::engine::DbExecutor, safe_cat: &str) -> Option<String> {
    if is_target_db_quack(exec) {
        return Some("target_db".to_string());
    }
    let cat_quack = exec
        .query_rows(&format!(
            "SELECT count(*) FROM duckdb_databases() WHERE internal = false AND database_name = '{}' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            safe_cat
        ))
        .ok()
        .and_then(|(_, rows)| rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
        > 0;
    if cat_quack {
        Some(safe_cat.to_string())
    } else {
        None
    }
}

fn current_database_exec(exec: &mut dyn crate::engine::DbExecutor) -> String {
    exec.query_rows("SELECT current_database()")
        .ok()
        .and_then(|(_, rows)| rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .filter(|s| !s.is_empty() && s != "NULL")
        .unwrap_or_else(|| "memory".to_string())
}

pub fn list_attached_databases(conn: &Connection) -> Result<Vec<AttachedDatabase>> {
    let mut exec = conn;
    list_attached_databases_exec(&mut exec)
}

pub fn list_attached_databases_exec(
    exec: &mut dyn crate::engine::DbExecutor,
) -> Result<Vec<AttachedDatabase>> {
    if is_target_db_quack(exec) {
        let quack_db_sql = "SELECT database_name, type, COALESCE(path, ''), readonly FROM target_db.query('SELECT database_name, type, path, readonly FROM duckdb_databases() WHERE internal = false')";
        if let Ok((_, rows)) = exec.query_rows(quack_db_sql) {
            let mut remote_dbs = Vec::new();
            for r in rows {
                if r.len() >= 4 {
                    remote_dbs.push(AttachedDatabase {
                        name: r[0].clone(),
                        db_type: r[1].clone(),
                        path: if r[2].is_empty() || r[2] == "NULL" {
                            None
                        } else {
                            Some(r[2].clone())
                        },
                        readonly: r[3].eq_ignore_ascii_case("true") || r[3] == "1",
                    });
                }
            }
            if !remote_dbs.is_empty() {
                return Ok(remote_dbs);
            }
        }
    }

    let (_, rows) = exec.query_rows("SELECT database_name, type, COALESCE(path, ''), readonly FROM duckdb_databases() WHERE internal = false")?;
    let mut dbs = Vec::new();
    for r in rows {
        if r.len() >= 4 {
            dbs.push(AttachedDatabase {
                name: r[0].clone(),
                db_type: r[1].clone(),
                path: if r[2].is_empty() || r[2] == "NULL" {
                    None
                } else {
                    Some(r[2].clone())
                },
                readonly: r[3].eq_ignore_ascii_case("true") || r[3] == "1",
            });
        }
    }
    // If other databases are attached to this connection (e.g. target_db or secondary catalogs),
    // and target_db represents an attached local file, filter out the client's internal default "memory" harness.
    if dbs.len() > 1 && dbs.iter().any(|d| d.name == "target_db") {
        dbs.retain(|d| d.name != "memory");
    }
    Ok(dbs)
}

pub fn validate_target_catalog(
    conn: &Connection,
    catalog: Option<&str>,
    for_apply: bool,
) -> Result<String> {
    let mut exec = conn;
    validate_target_catalog_exec(&mut exec, catalog, for_apply)
}

pub fn validate_target_catalog_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
    for_apply: bool,
) -> Result<String> {
    let attached = list_attached_databases_exec(exec).unwrap_or_default();
    let is_quack = is_target_db_quack(exec);

    let target = match catalog {
        Some(c) => c.to_string(),
        None => {
            if is_quack {
                exec.query_rows("SELECT * FROM target_db.query('SELECT current_database()')")
                    .ok()
                    .and_then(|(_, rows)| rows.into_iter().next())
                    .and_then(|r| r.into_iter().next())
                    .filter(|s| !s.is_empty() && s != "NULL")
                    .unwrap_or_else(|| "memory".to_string())
            } else {
                let curr = current_database_exec(exec);
                if curr == "memory"
                    && !attached
                        .iter()
                        .any(|d| d.name.eq_ignore_ascii_case("memory"))
                    && attached
                        .iter()
                        .any(|d| d.name.eq_ignore_ascii_case("target_db"))
                {
                    "target_db".to_string()
                } else {
                    curr
                }
            }
        }
    };

    if let Some(db_info) = attached
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(&target))
    {
        if for_apply {
            if db_info.readonly {
                anyhow::bail!(
                    "Cannot apply migrations to database '{}': attached in READ_ONLY mode.",
                    db_info.name
                );
            }
            let typ = db_info.db_type.to_lowercase();
            if typ != "duckdb" && typ != "quack" {
                anyhow::bail!(
                    "Cannot apply declarative schema migrations to '{}' (engine type: {}). \
                    DuckPack declarative schema management is designed specifically for DuckDB catalogs. \
                    External attached engines are supported for querying and DML, but schema DDL is managed externally.",
                    db_info.name,
                    db_info.db_type
                );
            }
        }
        Ok(db_info.name.clone())
    } else if attached.is_empty() {
        Ok(target)
    } else {
        let available: Vec<_> = attached.iter().map(|d| d.name.as_str()).collect();
        anyhow::bail!(
            "Attached database '{}' not found on connection. Available databases: {:?}",
            target,
            available
        );
    }
}

/// Common Table Expression for DuckDB 2.0 to resolve fully qualified nested schemas hierarchically.
pub const FULL_SCHEMAS_CTE: &str = "\
WITH RECURSIVE full_schemas AS ( \
    SELECT oid, database_name, schema_name, parent_schema_oid, schema_name AS full_schema_name, 0 AS depth \
    FROM duckdb_schemas() \
    WHERE parent_schema_oid IS NULL \
    UNION ALL \
    SELECT s.oid, s.database_name, s.schema_name, s.parent_schema_oid, f.full_schema_name || '.' || s.schema_name, f.depth + 1 \
    FROM duckdb_schemas() s \
    JOIN full_schemas f ON s.database_name = f.database_name AND s.parent_schema_oid = f.oid \
)";

/// Represents a dot-qualified SQL object identifier with support for arbitrary nested schemas.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedIdentifier {
    pub parts: Vec<String>,
}

impl QualifiedIdentifier {
    pub fn parse(name: &str) -> Self {
        let parts = name
            .split('.')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self { parts }
    }

    pub fn leaf_name(&self) -> &str {
        self.parts.last().map(|s| s.as_str()).unwrap_or("")
    }

    pub fn schema_prefix(&self) -> String {
        if self.parts.len() <= 1 {
            String::new()
        } else {
            let schema_parts = &self.parts[..self.parts.len() - 1];
            format!("{}.", schema_parts.join("."))
        }
    }

    pub fn full_schema(&self) -> Option<String> {
        if self.parts.len() <= 1 {
            None
        } else {
            Some(self.parts[..self.parts.len() - 1].join("."))
        }
    }

    pub fn quoted_qualified_name(&self) -> String {
        self.parts
            .iter()
            .map(|part| format!("\"{}\"", part.replace('\"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// Double-quotes each component of a dot-qualified identifier (e.g. `finance.reports` -> `"finance"."reports"`).
pub fn quote_qualified_identifier(name: &str) -> String {
    QualifiedIdentifier::parse(name).quoted_qualified_name()
}

/// Extracts the entire table schema from a DuckDB connection for a specific catalog.
/// It queries `duckdb_tables()` for table definitions and `duckdb_columns()` for specific datatypes.
/// Returns a HashMap mapping `schema.table` -> `TableSchema`.
pub fn get_schema(
    conn: &Connection,
    catalog: Option<&str>,
) -> Result<HashMap<String, TableSchema>> {
    let mut exec = conn;
    get_schema_exec(&mut exec, catalog)
}

pub fn get_schema_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<HashMap<String, TableSchema>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => current_database_exec(exec),
    };
    let safe_cat = target_cat.replace('\'', "''");
    let quack_alias = detect_quack_alias(exec, &safe_cat);

    let sql = format!(
        "{} SELECT CASE WHEN fs.full_schema_name = 'main' THEN t.table_name ELSE fs.full_schema_name || '.' || t.table_name END, \
         COALESCE(t.sql, '/* Remote table: ' || t.table_name || ' */') \
         FROM duckdb_tables() t \
         JOIN full_schemas fs ON t.schema_oid = fs.oid AND t.database_name = fs.database_name \
         WHERE t.internal = false AND t.database_name = '{}'",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let table_rows = if let Ok((_, rows)) = exec.query_rows(&sql) {
        rows
    } else {
        let fallback_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             COALESCE(sql, '/* Remote table: ' || table_name || ' */') \
             FROM duckdb_tables() WHERE internal = false AND database_name = '{}'",
            safe_cat
        );
        exec.query_rows(&fallback_sql)?.1
    };

    let mut tables: HashMap<String, TableSchema> = HashMap::new();
    for row in table_rows {
        if row.len() >= 2 {
            tables.insert(
                row[0].clone(),
                TableSchema {
                    create_sql: row[1].clone(),
                    columns: HashMap::new(),
                    column_defaults: HashMap::new(),
                    constraints: Vec::new(),
                },
            );
        }
    }

    if quack_alias.is_some() || tables.is_empty() {
        let q_alias = quack_alias.as_deref().unwrap_or("target_db");
        let q_sql = format!(
            "SELECT CASE WHEN fs.full_schema_name = 'main' THEN t.table_name ELSE fs.full_schema_name || '.' || t.table_name END, \
             COALESCE(t.sql, '/* Remote table: ' || t.table_name || ' */') \
             FROM {}.query('{} SELECT fs.full_schema_name, t.table_name, t.sql FROM duckdb_tables() t JOIN full_schemas fs ON t.schema_oid = fs.oid AND t.database_name = fs.database_name WHERE t.internal = false AND (t.database_name = ''{}'' OR t.database_name = current_database())')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_rows = if let Ok((_, rows)) = exec.query_rows(&q_sql) {
            Some(rows)
        } else {
            let fallback_q_sql = format!(
                "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
                 COALESCE(sql, '/* Remote table: ' || table_name || ' */') \
                 FROM {}.query('SELECT schema_name, table_name, sql FROM duckdb_tables() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database())')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_sql).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_rows {
            for row in rows {
                if row.len() >= 2 {
                    tables.entry(row[0].clone()).or_insert_with(|| TableSchema {
                        create_sql: row[1].clone(),
                        columns: HashMap::new(),
                        column_defaults: HashMap::new(),
                        constraints: Vec::new(),
                    });
                }
            }
        }
    }

    let col_sql = format!(
        "{} SELECT CASE WHEN fs.full_schema_name = 'main' THEN c.table_name ELSE fs.full_schema_name || '.' || c.table_name END, \
         c.column_name, c.data_type, \
         CASE WHEN c.column_default IS NULL THEN '0' ELSE '1' END, COALESCE(c.column_default, '') \
         FROM duckdb_columns() c \
         JOIN full_schemas fs ON c.schema_oid = fs.oid AND c.database_name = fs.database_name \
         WHERE c.internal = false AND c.database_name = '{}'",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let col_rows = if let Ok((_, rows)) = exec.query_rows(&col_sql) {
        rows
    } else {
        let fallback_col_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             column_name, data_type, \
             CASE WHEN column_default IS NULL THEN '0' ELSE '1' END, COALESCE(column_default, '') \
             FROM duckdb_columns() WHERE internal = false AND database_name = '{}'",
            safe_cat
        );
        exec.query_rows(&fallback_col_sql)?.1
    };

    for row in col_rows {
        if row.len() >= 5 {
            if let Some(table) = tables.get_mut(&row[0]) {
                table.columns.insert(row[1].clone(), row[2].clone());
                if row[3] == "1" {
                    table.column_defaults.insert(row[1].clone(), row[4].clone());
                }
            }
        }
    }

    let any_col_empty = tables.values().any(|t| t.columns.is_empty());
    if quack_alias.is_some() || any_col_empty {
        let q_alias = quack_alias.as_deref().unwrap_or("target_db");
        let q_col_sql = format!(
            "SELECT CASE WHEN fs.full_schema_name = 'main' THEN c.table_name ELSE fs.full_schema_name || '.' || c.table_name END, \
             c.column_name, c.data_type, \
             CASE WHEN c.column_default IS NULL THEN '0' ELSE '1' END, COALESCE(c.column_default, '') \
             FROM {}.query('{} SELECT fs.full_schema_name, c.table_name, c.column_name, c.data_type, c.column_default FROM duckdb_columns() c JOIN full_schemas fs ON c.schema_oid = fs.oid AND c.database_name = fs.database_name WHERE c.internal = false AND (c.database_name = ''{}'' OR c.database_name = current_database())')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_col_rows = if let Ok((_, rows)) = exec.query_rows(&q_col_sql) {
            Some(rows)
        } else {
            let fallback_q_col = format!(
                "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
                 column_name, data_type, \
                 CASE WHEN column_default IS NULL THEN '0' ELSE '1' END, COALESCE(column_default, '') \
                 FROM {}.query('SELECT schema_name, table_name, column_name, data_type, column_default FROM duckdb_columns() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database())')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_col).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_col_rows {
            for row in rows {
                if row.len() >= 5 {
                    if let Some(table) = tables.get_mut(&row[0]) {
                        table.columns.insert(row[1].clone(), row[2].clone());
                        if row[3] == "1" {
                            table.column_defaults.insert(row[1].clone(), row[4].clone());
                        } else {
                            table.column_defaults.remove(&row[1]);
                        }
                    }
                }
            }
        }
    }

    // Introspect table constraints (PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK)
    let constr_sql = format!(
        "{} SELECT CASE WHEN fs.full_schema_name = 'main' THEN c.table_name ELSE fs.full_schema_name || '.' || c.table_name END, \
         c.constraint_type, \
         COALESCE(c.constraint_name, ''), \
         COALESCE(array_to_string(c.constraint_column_names, ','), ''), \
         COALESCE(c.expression, ''), \
         COALESCE(c.referenced_table, ''), \
         COALESCE(array_to_string(c.referenced_column_names, ','), '') \
         FROM duckdb_constraints() c \
         JOIN full_schemas fs ON c.schema_oid = fs.oid AND c.database_name = fs.database_name \
         WHERE (c.database_name = '{}' OR c.database_name = 'target_db') AND c.constraint_type IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY', 'CHECK')",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let constr_rows = if let Ok((_, rows)) = exec.query_rows(&constr_sql) {
        Some(rows)
    } else {
        let fallback_constr_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             constraint_type, \
             COALESCE(constraint_name, ''), \
             COALESCE(array_to_string(constraint_column_names, ','), ''), \
             COALESCE(expression, ''), \
             COALESCE(referenced_table, ''), \
             COALESCE(array_to_string(referenced_column_names, ','), '') \
             FROM duckdb_constraints() \
             WHERE (database_name = '{}' OR database_name = 'target_db') AND constraint_type IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY', 'CHECK')",
            safe_cat
        );
        exec.query_rows(&fallback_constr_sql).ok().map(|(_, r)| r)
    };
    if let Some(rows) = constr_rows {
        for row in rows {
            if row.len() >= 7 {
                if let Some(table) = tables.get_mut(&row[0]) {
                    let cols: Vec<String> = if row[3].is_empty() {
                        Vec::new()
                    } else {
                        row[3]
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    };
                    let ref_cols: Vec<String> = if row[6].is_empty() {
                        Vec::new()
                    } else {
                        row[6]
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    };
                    table.constraints.push(ConstraintSchema {
                        constraint_type: row[1].clone(),
                        constraint_name: if row[2].is_empty() {
                            None
                        } else {
                            Some(row[2].clone())
                        },
                        columns: cols,
                        expression: if row[4].is_empty() {
                            None
                        } else {
                            Some(row[4].clone())
                        },
                        referenced_table: if row[5].is_empty() {
                            None
                        } else {
                            Some(row[5].clone())
                        },
                        referenced_columns: ref_cols,
                    });
                }
            }
        }
    }

    if let Some(ref q_alias) = quack_alias {
        let q_constr_sql = format!(
            "SELECT CASE WHEN fs.full_schema_name = 'main' THEN c.table_name ELSE fs.full_schema_name || '.' || c.table_name END, \
             constraint_type, \
             COALESCE(constraint_name, ''), \
             COALESCE(constraint_column_names, ''), \
             COALESCE(expression, ''), \
             COALESCE(referenced_table, ''), \
             COALESCE(referenced_column_names, '') \
             FROM {}.query('{} SELECT fs.full_schema_name, c.table_name, c.constraint_type, c.constraint_name, array_to_string(c.constraint_column_names, '','') as constraint_column_names, c.expression, c.referenced_table, array_to_string(c.referenced_column_names, '','') as referenced_column_names FROM duckdb_constraints() c JOIN full_schemas fs ON c.schema_oid = fs.oid AND c.database_name = fs.database_name WHERE c.constraint_type IN (''PRIMARY KEY'', ''UNIQUE'', ''FOREIGN KEY'', ''CHECK'') AND (c.database_name = ''{}'' OR c.database_name = current_database())')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_rows = if let Ok((_, rows)) = exec.query_rows(&q_constr_sql) {
            Some(rows)
        } else {
            let fallback_q_constr = format!(
                "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
                 constraint_type, \
                 COALESCE(constraint_name, ''), \
                 COALESCE(constraint_column_names, ''), \
                 COALESCE(expression, ''), \
                 COALESCE(referenced_table, ''), \
                 COALESCE(referenced_column_names, '') \
                 FROM {}.query('SELECT schema_name, table_name, constraint_type, constraint_name, array_to_string(constraint_column_names, '','') as constraint_column_names, expression, referenced_table, array_to_string(referenced_column_names, '','') as referenced_column_names FROM duckdb_constraints() WHERE constraint_type IN (''PRIMARY KEY'', ''UNIQUE'', ''FOREIGN KEY'', ''CHECK'') AND (database_name = ''{}'' OR database_name = current_database())')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_constr).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_rows {
            for row in rows {
                if row.len() >= 7 {
                    if let Some(table) = tables.get_mut(&row[0]) {
                        let cols: Vec<String> = if row[3].is_empty() {
                            Vec::new()
                        } else {
                            row[3]
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        };
                        let ref_cols: Vec<String> = if row[6].is_empty() {
                            Vec::new()
                        } else {
                            row[6]
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        };
                        let new_c = ConstraintSchema {
                            constraint_type: row[1].clone(),
                            constraint_name: if row[2].is_empty() {
                                None
                            } else {
                                Some(row[2].clone())
                            },
                            columns: cols,
                            expression: if row[4].is_empty() {
                                None
                            } else {
                                Some(row[4].clone())
                            },
                            referenced_table: if row[5].is_empty() {
                                None
                            } else {
                                Some(row[5].clone())
                            },
                            referenced_columns: ref_cols,
                        };
                        if !table.constraints.iter().any(|c| c.matches(&new_c)) {
                            table.constraints.push(new_c);
                        }
                    }
                }
            }
        }
    }

    Ok(tables)
}

/// Extracts all view definitions from a DuckDB connection for a specific catalog.
pub fn get_views(conn: &Connection, catalog: Option<&str>) -> Result<HashMap<String, String>> {
    let mut exec = conn;
    get_views_exec(&mut exec, catalog)
}

pub fn get_views_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<HashMap<String, String>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => current_database_exec(exec),
    };
    let safe_cat = target_cat.replace('\'', "''");
    let quack_alias = detect_quack_alias(exec, &safe_cat);

    let sql = format!(
        "{} SELECT CASE WHEN fs.full_schema_name = 'main' THEN v.view_name ELSE fs.full_schema_name || '.' || v.view_name END, \
         COALESCE(v.sql, '/* Remote view: ' || v.view_name || ' */') \
         FROM duckdb_views() v \
         JOIN full_schemas fs ON v.schema_oid = fs.oid AND v.database_name = fs.database_name \
         WHERE v.internal = false AND v.database_name = '{}'",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let mut views = HashMap::new();
    let view_rows = if let Ok((_, rows)) = exec.query_rows(&sql) {
        Some(rows)
    } else {
        let fallback_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, \
             COALESCE(sql, '/* Remote view: ' || view_name || ' */') \
             FROM duckdb_views() WHERE internal = false AND database_name = '{}'",
            safe_cat
        );
        exec.query_rows(&fallback_sql).ok().map(|(_, r)| r)
    };
    if let Some(rows) = view_rows {
        for row in rows {
            if row.len() >= 2 {
                views.insert(row[0].clone(), row[1].clone());
            }
        }
    }

    if views.is_empty() {
        let q_alias = quack_alias.as_deref().unwrap_or("target_db");
        let q_sql = format!(
            "SELECT CASE WHEN fs.full_schema_name = 'main' THEN v.view_name ELSE fs.full_schema_name || '.' || v.view_name END, \
             COALESCE(v.sql, '/* Remote view: ' || v.view_name || ' */') \
             FROM {}.query('{} SELECT fs.full_schema_name, v.view_name, v.sql FROM duckdb_views() v JOIN full_schemas fs ON v.schema_oid = fs.oid AND v.database_name = fs.database_name WHERE v.internal = false AND (v.database_name = ''{}'' OR v.database_name = current_database())')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_rows = if let Ok((_, rows)) = exec.query_rows(&q_sql) {
            Some(rows)
        } else {
            let fallback_q_sql = format!(
                "SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, \
                 COALESCE(sql, '/* Remote view: ' || view_name || ' */') \
                 FROM {}.query('SELECT schema_name, view_name, sql FROM duckdb_views() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database())')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_sql).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_rows {
            for row in rows {
                if row.len() >= 2 {
                    views.insert(row[0].clone(), row[1].clone());
                }
            }
        }
    }
    Ok(views)
}

/// Extracts all macro (function) definitions from a DuckDB connection for a specific catalog.
pub fn get_macros(conn: &Connection, catalog: Option<&str>) -> Result<HashMap<String, String>> {
    let mut exec = conn;
    get_macros_exec(&mut exec, catalog)
}

pub fn get_macros_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<HashMap<String, String>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => current_database_exec(exec),
    };
    let safe_cat = target_cat.replace('\'', "''");

    let sql = format!(
        "{} SELECT CASE WHEN COALESCE(fs.full_schema_name, f.schema_name) = 'main' THEN f.function_name ELSE COALESCE(fs.full_schema_name, f.schema_name) || '.' || f.function_name END, \
         COALESCE(f.macro_definition, ''), COALESCE(CAST(f.parameters AS VARCHAR), '') \
         FROM duckdb_functions() f \
         LEFT JOIN full_schemas fs ON f.schema_name = fs.schema_name AND f.database_name = fs.database_name \
         WHERE f.function_type = 'macro' AND f.internal = false AND f.database_name = '{}'",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let macro_rows = if let Ok((_, rows)) = exec.query_rows(&sql) {
        rows
    } else {
        let fallback_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END, \
             COALESCE(macro_definition, ''), COALESCE(CAST(parameters AS VARCHAR), '') \
             FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false AND database_name = '{}'",
            safe_cat
        );
        exec.query_rows(&fallback_sql)?.1
    };

    let mut macros = HashMap::new();
    for row in macro_rows {
        if row.len() >= 3 {
            let params_clean = row[2].trim_matches(|c| c == '[' || c == ']');
            let sql = format!("CREATE MACRO {}({}) AS {};", row[0], params_clean, row[1]);
            macros.insert(row[0].clone(), sql);
        }
    }
    Ok(macros)
}

/// Extracts all schema names (excluding system schemas) from a DuckDB connection for a specific catalog.
/// Returns fully-qualified nested schema names hierarchically sorted.
pub fn get_schemas(
    conn: &Connection,
    catalog: Option<&str>,
) -> Result<std::collections::HashSet<String>> {
    let mut exec = conn;
    get_schemas_exec(&mut exec, catalog)
}

pub fn get_schemas_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<std::collections::HashSet<String>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => current_database_exec(exec),
    };
    let safe_cat = target_cat.replace('\'', "''");
    let quack_alias = detect_quack_alias(exec, &safe_cat);

    let sql = format!(
        "{} SELECT full_schema_name FROM full_schemas WHERE database_name = '{}' AND full_schema_name NOT IN ('information_schema', 'pg_catalog', 'main') ORDER BY depth ASC",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let mut schemas = std::collections::HashSet::new();
    let schema_rows = if let Ok((_, rows)) = exec.query_rows(&sql) {
        Some(rows)
    } else {
        let fallback_sql = format!(
            "SELECT schema_name FROM duckdb_schemas() WHERE internal = false AND database_name = '{}' AND schema_name NOT IN ('information_schema', 'pg_catalog', 'main')",
            safe_cat
        );
        exec.query_rows(&fallback_sql).ok().map(|(_, r)| r)
    };
    if let Some(rows) = schema_rows {
        for row in rows {
            if let Some(name) = row.into_iter().next() {
                schemas.insert(name);
            }
        }
    }

    if schemas.is_empty() {
        let q_alias = quack_alias.as_deref().unwrap_or("target_db");
        let q_sql = format!(
            "SELECT full_schema_name FROM {}.query('{} SELECT full_schema_name FROM full_schemas WHERE (database_name = ''{}'' OR database_name = current_database()) AND full_schema_name NOT IN (''information_schema'', ''pg_catalog'', ''main'') ORDER BY depth ASC')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_rows = if let Ok((_, rows)) = exec.query_rows(&q_sql) {
            Some(rows)
        } else {
            let fallback_q_sql = format!(
                "SELECT schema_name FROM {}.query('SELECT schema_name FROM duckdb_schemas() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database()) AND schema_name NOT IN (''information_schema'', ''pg_catalog'', ''main'')')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_sql).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_rows {
            for row in rows {
                if let Some(name) = row.into_iter().next() {
                    schemas.insert(name);
                }
            }
        }
    }
    Ok(schemas)
}

/// Extracts all trigger definitions from a DuckDB connection for a specific catalog.
pub fn get_triggers(
    conn: &Connection,
    catalog: Option<&str>,
) -> Result<HashMap<String, TriggerSchema>> {
    let mut exec = conn;
    get_triggers_exec(&mut exec, catalog)
}

pub fn get_triggers_exec(
    exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<HashMap<String, TriggerSchema>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => current_database_exec(exec),
    };
    let safe_cat = target_cat.replace('\'', "''");
    let quack_alias = detect_quack_alias(exec, &safe_cat);
    let mut triggers = HashMap::new();

    let sql = format!(
        "{} SELECT fs.full_schema_name, t.trigger_name, t.table_name, COALESCE(t.sql, '') \
         FROM duckdb_triggers() t \
         JOIN full_schemas fs ON t.schema_oid = fs.oid AND t.database_name = fs.database_name \
         WHERE t.database_name = '{}'",
        FULL_SCHEMAS_CTE, safe_cat
    );
    let trg_rows = if let Ok((_, rows)) = exec.query_rows(&sql) {
        Some(rows)
    } else {
        let fallback_sql = format!(
            "SELECT schema_name, trigger_name, table_name, COALESCE(sql, '') \
             FROM duckdb_triggers() WHERE database_name = '{}'",
            safe_cat
        );
        exec.query_rows(&fallback_sql).ok().map(|(_, r)| r)
    };
    if let Some(rows) = trg_rows {
        for row in rows {
            if row.len() >= 4 {
                let schema = row[0].clone();
                let trg_name = row[1].clone();
                let table = row[2].clone();
                let trg_sql = row[3].clone();
                let key = if schema == "main" {
                    trg_name.clone()
                } else {
                    format!("{}.{}", schema, trg_name)
                };
                triggers.insert(
                    key,
                    TriggerSchema {
                        schema_name: schema,
                        trigger_name: trg_name,
                        table_name: table,
                        sql: trg_sql,
                    },
                );
            }
        }
    }

    if triggers.is_empty() {
        let q_alias = quack_alias.as_deref().unwrap_or("target_db");
        let q_sql = format!(
            "SELECT fs.full_schema_name, t.trigger_name, t.table_name, COALESCE(t.sql, '') \
             FROM {}.query('{} SELECT fs.full_schema_name, t.trigger_name, t.table_name, t.sql FROM duckdb_triggers() t JOIN full_schemas fs ON t.schema_oid = fs.oid AND t.database_name = fs.database_name WHERE (t.database_name = ''{}'' OR t.database_name = current_database())')",
            q_alias,
            FULL_SCHEMAS_CTE.replace('\'', "''"),
            safe_cat
        );
        let q_rows = if let Ok((_, rows)) = exec.query_rows(&q_sql) {
            Some(rows)
        } else {
            let fallback_q_sql = format!(
                "SELECT schema_name, trigger_name, table_name, COALESCE(sql, '') \
                 FROM {}.query('SELECT schema_name, trigger_name, table_name, sql FROM duckdb_triggers() WHERE database_name = ''{}''')",
                q_alias, safe_cat
            );
            exec.query_rows(&fallback_q_sql).ok().map(|(_, r)| r)
        };
        if let Some(rows) = q_rows {
            for row in rows {
                if row.len() >= 4 {
                    let schema = row[0].clone();
                    let trg_name = row[1].clone();
                    let table = row[2].clone();
                    let trg_sql = row[3].clone();
                    let key = if schema == "main" {
                        trg_name.clone()
                    } else {
                        format!("{}.{}", schema, trg_name)
                    };
                    triggers.insert(
                        key,
                        TriggerSchema {
                            schema_name: schema,
                            trigger_name: trg_name,
                            table_name: table,
                            sql: trg_sql,
                        },
                    );
                }
            }
        }
    }

    Ok(triggers)
}

/// Compares two `CatalogSnapshot` instances and calculates the declarative migration plan (`DiffResult`).
pub fn compute_diff_snapshots(
    shadow: &CatalogSnapshot,
    target: &CatalogSnapshot,
    project_dir: &Path,
    ignore_list: &[String],
) -> DiffResult {
    compute_diff(
        &shadow.schemas,
        &target.schemas,
        &shadow.tables,
        &target.tables,
        &shadow.views,
        &target.views,
        &shadow.macros,
        &target.macros,
        &shadow.triggers,
        &target.triggers,
        &project_dir.to_path_buf(),
        ignore_list,
    )
}

/// High-level deep interface: compiles `project_dir` into an in-memory Shadow Database,
/// introspects both Shadow and Target catalogs over `DbExecutor`, loads `.duckpackignore`,
/// and returns the computed `DiffResult`.
pub fn diff_project(
    project_dir: &Path,
    target_exec: &mut dyn crate::engine::DbExecutor,
    catalog: Option<&str>,
) -> Result<DiffResult> {
    let (_shadow_conn, shadow_snapshot) = CatalogSnapshot::from_project_dir(project_dir)?;
    let target_snapshot = CatalogSnapshot::introspect(target_exec, catalog)?;
    let ignore_list = crate::engine::load_ignore_rules(project_dir);
    Ok(compute_diff_snapshots(
        &shadow_snapshot,
        &target_snapshot,
        project_dir,
        &ignore_list,
    ))
}

/// The core diffing engine.
/// Compares the `shadow` database (desired state) against the `target` database (current state)
/// and calculates the exact delta (adding tables, renaming columns, altering types, triggers, etc.).
/// It uses Jaro-Winkler string similarity to heuristically detect renames instead of destructive drops.
pub fn compute_diff(
    shadow_schemas: &std::collections::HashSet<String>,
    target_schemas: &std::collections::HashSet<String>,
    shadow: &HashMap<String, TableSchema>,
    target: &HashMap<String, TableSchema>,
    shadow_views: &HashMap<String, String>,
    target_views: &HashMap<String, String>,
    shadow_macros: &HashMap<String, String>,
    target_macros: &HashMap<String, String>,
    shadow_triggers: &HashMap<String, TriggerSchema>,
    target_triggers: &HashMap<String, TriggerSchema>,
    _project_dir: &std::path::PathBuf,
    ignore_list: &[String],
) -> DiffResult {
    let mut diff = DiffResult { items: Vec::new() };

    let is_ignored = |name: &str| -> bool {
        ignore_list.iter().any(|pattern| {
            if pattern.ends_with('*') {
                let prefix = &pattern[..pattern.len() - 1];
                name.starts_with(prefix)
            } else {
                name == pattern
            }
        })
    };

    let mut created_tables = Vec::new();
    let mut dropped_tables = Vec::new();

    // 1. Schemas
    let mut schemas_to_create: Vec<String> = shadow_schemas
        .iter()
        .filter(|s| !is_ignored(s) && !target_schemas.contains(*s))
        .cloned()
        .collect();
    // Sort topologically: parent schemas (fewer dots) before nested child schemas (more dots)
    schemas_to_create.sort_by(|a, b| {
        let depth_a = a.matches('.').count();
        let depth_b = b.matches('.').count();
        depth_a.cmp(&depth_b).then_with(|| a.cmp(b))
    });

    for schema_name in schemas_to_create {
        let quoted_name = quote_qualified_identifier(&schema_name);
        diff.items.push(DiffItem {
            item_type: DiffItemType::CreateSchema,
            obj_name: schema_name.clone(),
            sql: format!("CREATE SCHEMA {};", quoted_name),
            old_def: None,
            new_def: None,
            selected: true,
            locked: false,
            ignored: false,
        });
    }

    let mut schemas_to_drop: Vec<String> = target_schemas
        .iter()
        .filter(|s| !is_ignored(s) && !shadow_schemas.contains(*s))
        .cloned()
        .collect();
    // For dropping, sort child schemas first (more dots first)
    schemas_to_drop.sort_by(|a, b| {
        let depth_a = a.matches('.').count();
        let depth_b = b.matches('.').count();
        depth_b.cmp(&depth_a).then_with(|| a.cmp(b))
    });

    for schema_name in schemas_to_drop {
        let quoted_name = quote_qualified_identifier(&schema_name);
        diff.items.push(DiffItem {
            item_type: DiffItemType::DropSchema,
            obj_name: schema_name.clone(),
            sql: format!("DROP SCHEMA {} CASCADE;", quoted_name),
            old_def: None,
            new_def: None,
            selected: true,
            locked: false,
            ignored: false,
        });
    }

    for (table_name, shadow_table) in shadow.iter() {
        if let Some(target_table) = target.get(table_name) {
            let mut safe_sqls = Vec::new();
            let mut destructive_sqls = Vec::new();

            let mut created_cols = Vec::new();
            let mut dropped_cols = Vec::new();

            for (col_name, _) in shadow_table.columns.iter() {
                if !target_table.columns.contains_key(col_name) {
                    created_cols.push(col_name.clone());
                }
            }

            for (col_name, _) in target_table.columns.iter() {
                if !shadow_table.columns.contains_key(col_name) {
                    dropped_cols.push(col_name.clone());
                }
            }

            let mut matched_created_cols = std::collections::HashSet::new();
            let mut matched_dropped_cols = std::collections::HashSet::new();

            for dropped in &dropped_cols {
                for created in &created_cols {
                    if matched_created_cols.contains(created) {
                        continue;
                    }
                    if strsim::jaro_winkler(dropped, created) > 0.8 {
                        matched_dropped_cols.insert(dropped.clone());
                        matched_created_cols.insert(created.clone());
                        safe_sqls.push(format!(
                            "ALTER TABLE {} RENAME COLUMN {} TO {};",
                            table_name, dropped, created
                        ));
                        break;
                    }
                }
            }

            for col_name in &created_cols {
                if !matched_created_cols.contains(col_name) {
                    let shadow_type = shadow_table.columns.get(col_name).unwrap();
                    if let Some(def_val) = shadow_table.column_defaults.get(col_name) {
                        safe_sqls.push(format!(
                            "ALTER TABLE {} ADD COLUMN {} {} DEFAULT {};",
                            table_name, col_name, shadow_type, def_val
                        ));
                    } else {
                        safe_sqls.push(format!(
                            "ALTER TABLE {} ADD COLUMN {} {};",
                            table_name, col_name, shadow_type
                        ));
                    }
                }
            }

            for col_name in &dropped_cols {
                if !matched_dropped_cols.contains(col_name) {
                    destructive_sqls.push(format!(
                        "ALTER TABLE {} DROP COLUMN {};",
                        table_name, col_name
                    ));
                }
            }

            for (col_name, shadow_type) in shadow_table.columns.iter() {
                if let Some(target_type) = target_table.columns.get(col_name) {
                    if shadow_type != target_type {
                        safe_sqls.push(format!(
                            "ALTER TABLE {} ALTER {} TYPE {};",
                            table_name, col_name, shadow_type
                        ));
                    }

                    let shadow_def = shadow_table.column_defaults.get(col_name);
                    let target_def = target_table.column_defaults.get(col_name);

                    match (shadow_def, target_def) {
                        (Some(s_def), Some(t_def)) => {
                            if !s_def.trim().eq_ignore_ascii_case(t_def.trim()) {
                                safe_sqls.push(format!(
                                    "ALTER TABLE {} ALTER {} SET DEFAULT {};",
                                    table_name, col_name, s_def
                                ));
                            }
                        }
                        (Some(s_def), None) => {
                            safe_sqls.push(format!(
                                "ALTER TABLE {} ALTER {} SET DEFAULT {};",
                                table_name, col_name, s_def
                            ));
                        }
                        (None, Some(_)) => {
                            safe_sqls.push(format!(
                                "ALTER TABLE {} ALTER {} DROP DEFAULT;",
                                table_name, col_name
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }

            let mut added_constraints = Vec::new();
            for sc in &shadow_table.constraints {
                if !target_table.constraints.iter().any(|tc| tc.matches(sc)) {
                    added_constraints.push(sc);
                }
            }

            let mut dropped_constraints = Vec::new();
            for tc in &target_table.constraints {
                if !shadow_table.constraints.iter().any(|sc| sc.matches(tc)) {
                    dropped_constraints.push(tc);
                }
            }

            let mut needs_recreate = false;
            let mut in_place_constraints = Vec::new();

            if !dropped_constraints.is_empty() {
                needs_recreate = true;
            }

            for ac in &added_constraints {
                match ac.constraint_type.to_uppercase().as_str() {
                    "PRIMARY KEY" => {
                        let target_has_pk = target_table
                            .constraints
                            .iter()
                            .any(|c| c.constraint_type.eq_ignore_ascii_case("PRIMARY KEY"));
                        if target_has_pk {
                            needs_recreate = true;
                        } else {
                            in_place_constraints.push(format!(
                                "ALTER TABLE {} ADD PRIMARY KEY ({});",
                                table_name,
                                ac.columns.join(", ")
                            ));
                        }
                    }
                    "UNIQUE" => {
                        in_place_constraints.push(format!(
                            "ALTER TABLE {} ADD UNIQUE ({});",
                            table_name,
                            ac.columns.join(", ")
                        ));
                    }
                    "FOREIGN KEY" | "CHECK" => {
                        needs_recreate = true;
                    }
                    _ => {}
                }
            }

            if needs_recreate {
                let ident = QualifiedIdentifier::parse(table_name);
                let schema_prefix = ident.schema_prefix();
                let simple_name = ident.leaf_name();
                let swap_table_name = format!("{}{}__duckpack_swap", schema_prefix, simple_name);

                let create_table_re = Regex::new(r"(?i)^\s*CREATE\s+TABLE\s+[^\s(]+").unwrap();
                let swap_create_sql = create_table_re.replace(
                    &shadow_table.create_sql,
                    format!("CREATE TABLE {}", swap_table_name),
                );

                let mut common_cols: Vec<String> = shadow_table
                    .columns
                    .keys()
                    .filter(|col| target_table.columns.contains_key(*col))
                    .cloned()
                    .collect();
                common_cols.sort();

                let mut swap_script = String::new();
                swap_script.push_str("BEGIN TRANSACTION;\n");
                let swap_create = swap_create_sql.trim().trim_end_matches(';');
                swap_script.push_str(swap_create);
                swap_script.push_str(";\n");

                if !common_cols.is_empty() {
                    let quoted_cols: Vec<String> =
                        common_cols.iter().map(|c| format!("\"{}\"", c)).collect();
                    let cols_str = quoted_cols.join(", ");
                    swap_script.push_str(&format!(
                        "INSERT INTO {} ({}) SELECT {} FROM {};\n",
                        swap_table_name, cols_str, cols_str, table_name
                    ));
                }

                let quoted_table = quote_qualified_identifier(table_name);
                let quoted_swap = quote_qualified_identifier(&swap_table_name);
                let quoted_simple = format!("\"{}\"", simple_name.replace('\"', "\"\""));

                swap_script.push_str(&format!("DROP TABLE {};\n", quoted_table));
                swap_script.push_str(&format!(
                    "ALTER TABLE {} RENAME TO {};\n",
                    quoted_swap, quoted_simple
                ));
                swap_script.push_str("COMMIT;");

                let ignored = is_ignored(table_name);
                diff.items.push(DiffItem {
                    item_type: DiffItemType::RecreateTable,
                    obj_name: table_name.clone(),
                    sql: swap_script,
                    old_def: Some(target_table.create_sql.clone()),
                    new_def: Some(shadow_table.create_sql.clone()),
                    selected: !ignored,
                    locked: false,
                    ignored,
                });

                // Auto-preserve triggers attached to this recreated table
                for (trg_name, shadow_trg) in shadow_triggers {
                    let trg_table_ident = QualifiedIdentifier::parse(&shadow_trg.table_name);
                    if trg_table_ident.leaf_name() == ident.leaf_name()
                        && (trg_table_ident.full_schema().is_none()
                            || trg_table_ident.full_schema() == ident.full_schema())
                    {
                        if !diff.items.iter().any(|item| {
                            item.item_type == DiffItemType::CreateTrigger
                                && item.obj_name == *trg_name
                        }) {
                            let is_trg_ignored = is_ignored(trg_name) || is_ignored(table_name);
                            diff.items.push(DiffItem {
                                item_type: DiffItemType::CreateTrigger,
                                obj_name: trg_name.clone(),
                                sql: shadow_trg.sql.clone(),
                                old_def: None,
                                new_def: Some(shadow_trg.sql.clone()),
                                selected: !is_trg_ignored,
                                locked: false,
                                ignored: is_trg_ignored,
                            });
                        }
                    }
                }
            } else {
                if !safe_sqls.is_empty() {
                    let has_rename = safe_sqls.iter().any(|s| s.contains("RENAME COLUMN"));
                    let ignored = is_ignored(table_name);
                    diff.items.push(DiffItem {
                        item_type: if has_rename {
                            DiffItemType::RenameColumn
                        } else {
                            DiffItemType::AlterTable
                        },
                        obj_name: table_name.clone(),
                        sql: safe_sqls.join("\n"),
                        old_def: Some(target_table.create_sql.clone()),
                        new_def: Some(shadow_table.create_sql.clone()),
                        selected: !ignored,
                        locked: false,
                        ignored,
                    });
                }

                if !in_place_constraints.is_empty() {
                    let ignored = is_ignored(table_name);
                    diff.items.push(DiffItem {
                        item_type: DiffItemType::AddConstraint,
                        obj_name: table_name.clone(),
                        sql: in_place_constraints.join("\n"),
                        old_def: Some(target_table.create_sql.clone()),
                        new_def: Some(shadow_table.create_sql.clone()),
                        selected: !ignored,
                        locked: false,
                        ignored,
                    });
                }

                if !destructive_sqls.is_empty() {
                    let ignored = is_ignored(table_name);
                    diff.items.push(DiffItem {
                        item_type: DiffItemType::DropColumn,
                        obj_name: table_name.clone(),
                        sql: destructive_sqls.join("\n"),
                        old_def: Some(target_table.create_sql.clone()),
                        new_def: Some(shadow_table.create_sql.clone()),
                        selected: !ignored,
                        locked: false,
                        ignored,
                    });
                }
            }
        } else {
            created_tables.push(table_name.clone());
        }
    }

    for table_name in target.keys() {
        if !shadow.contains_key(table_name) {
            dropped_tables.push(table_name.clone());
        }
    }

    let mut matched_created_tables = std::collections::HashSet::new();
    let mut matched_dropped_tables = std::collections::HashSet::new();

    for dropped in &dropped_tables {
        for created in &created_tables {
            if matched_created_tables.contains(created) {
                continue;
            }
            if strsim::jaro_winkler(dropped, created) > 0.8 {
                matched_dropped_tables.insert(dropped.clone());
                matched_created_tables.insert(created.clone());
                let shadow_table = shadow.get(created).unwrap();
                let target_table = target.get(dropped).unwrap();
                let ignored = is_ignored(dropped) || is_ignored(created);
                diff.items.push(DiffItem {
                    item_type: DiffItemType::RenameTable,
                    obj_name: format!("{} -> {}", dropped, created),
                    sql: format!("ALTER TABLE {} RENAME TO {};", dropped, created),
                    old_def: Some(target_table.create_sql.clone()),
                    new_def: Some(shadow_table.create_sql.clone()),
                    selected: !ignored,
                    locked: false,
                    ignored,
                });
                break;
            }
        }
    }

    for created in &created_tables {
        if !matched_created_tables.contains(created) {
            let shadow_table = shadow.get(created).unwrap();
            let ignored = is_ignored(created);
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateTable,
                obj_name: created.clone(),
                sql: shadow_table.create_sql.clone(),
                old_def: Some(String::new()),
                new_def: Some(shadow_table.create_sql.clone()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    for dropped in &dropped_tables {
        if !matched_dropped_tables.contains(dropped) {
            let ignored = is_ignored(dropped);
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropTable,
                obj_name: dropped.clone(),
                sql: format!("DROP TABLE {}", dropped),
                old_def: Some(target.get(dropped).unwrap().create_sql.clone()),
                new_def: Some(String::new()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    // Add views and macros
    for (name, shadow_def) in shadow_views {
        let target_def = target_views.get(name);

        if target_def.is_none() || target_def != Some(shadow_def) {
            let view_re =
                Regex::new(r"(?i)^(\s*CREATE\s+)(?:OR\s+REPLACE\s+)?(?:SECURE\s+)?VIEW\b").unwrap();
            let is_secure = Regex::new(r"(?i)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?SECURE\s+VIEW\b")
                .unwrap()
                .is_match(shadow_def);
            let safe_sql = if is_secure {
                view_re
                    .replace(shadow_def, "${1}OR REPLACE SECURE VIEW")
                    .to_string()
            } else {
                view_re
                    .replace(shadow_def, "${1}OR REPLACE VIEW")
                    .to_string()
            };
            let ignored = is_ignored(name);

            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateView,
                obj_name: name.clone(),
                sql: safe_sql,
                old_def: target_def.cloned(),
                new_def: Some(shadow_def.clone()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    for name in target_views.keys() {
        if !shadow_views.contains_key(name) {
            let ignored = is_ignored(name);
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropView,
                obj_name: name.clone(),
                sql: format!("DROP VIEW {}", name),
                old_def: Some(target_views.get(name).unwrap().clone()),
                new_def: Some(String::new()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    for (name, shadow_def) in shadow_macros {
        let target_def = target_macros.get(name);

        if target_def.is_none() || target_def != Some(shadow_def) {
            let macro_re = Regex::new(r"(?i)^(\s*CREATE\s+)MACRO\b").unwrap();
            let safe_sql = macro_re
                .replace(shadow_def, "${1}OR REPLACE MACRO")
                .to_string();
            let ignored = is_ignored(name);

            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateMacro,
                obj_name: name.clone(),
                sql: safe_sql,
                old_def: target_def.cloned(),
                new_def: Some(shadow_def.clone()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    for name in target_macros.keys() {
        if !shadow_macros.contains_key(name) {
            let ignored = is_ignored(name);
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropMacro,
                obj_name: name.clone(),
                sql: format!("DROP MACRO {}", name),
                old_def: Some(target_macros.get(name).unwrap().clone()),
                new_def: Some(String::new()),
                selected: !ignored,
                locked: false,
                ignored,
            });
        }
    }

    // 5. Triggers
    for (name, shadow_trg) in shadow_triggers {
        let is_trg_ignored = is_ignored(name) || is_ignored(&shadow_trg.table_name);
        if let Some(target_trg) = target_triggers.get(name) {
            if shadow_trg.sql.trim() != target_trg.sql.trim() {
                let target_table = if target_trg.schema_name == "main" {
                    quote_qualified_identifier(&target_trg.table_name)
                } else {
                    quote_qualified_identifier(&format!(
                        "{}.{}",
                        target_trg.schema_name, target_trg.table_name
                    ))
                };
                let quoted_trg = format!("\"{}\"", target_trg.trigger_name.replace('\"', "\"\""));
                diff.items.push(DiffItem {
                    item_type: DiffItemType::DropTrigger,
                    obj_name: name.clone(),
                    sql: format!("DROP TRIGGER {} ON {};", quoted_trg, target_table),
                    old_def: Some(target_trg.sql.clone()),
                    new_def: None,
                    selected: !is_trg_ignored,
                    locked: false,
                    ignored: is_trg_ignored,
                });
                diff.items.push(DiffItem {
                    item_type: DiffItemType::CreateTrigger,
                    obj_name: name.clone(),
                    sql: shadow_trg.sql.clone(),
                    old_def: None,
                    new_def: Some(shadow_trg.sql.clone()),
                    selected: !is_trg_ignored,
                    locked: false,
                    ignored: is_trg_ignored,
                });
            }
        } else {
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateTrigger,
                obj_name: name.clone(),
                sql: shadow_trg.sql.clone(),
                old_def: None,
                new_def: Some(shadow_trg.sql.clone()),
                selected: !is_trg_ignored,
                locked: false,
                ignored: is_trg_ignored,
            });
        }
    }

    for (name, target_trg) in target_triggers {
        if !shadow_triggers.contains_key(name) {
            let is_trg_ignored = is_ignored(name) || is_ignored(&target_trg.table_name);
            let target_table = if target_trg.schema_name == "main" {
                quote_qualified_identifier(&target_trg.table_name)
            } else {
                quote_qualified_identifier(&format!(
                    "{}.{}",
                    target_trg.schema_name, target_trg.table_name
                ))
            };
            let quoted_trg = format!("\"{}\"", target_trg.trigger_name.replace('\"', "\"\""));
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropTrigger,
                obj_name: name.clone(),
                sql: format!("DROP TRIGGER {} ON {};", quoted_trg, target_table),
                old_def: Some(target_trg.sql.clone()),
                new_def: None,
                selected: !is_trg_ignored,
                locked: false,
                ignored: is_trg_ignored,
            });
        }
    }

    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_compute_diff_new_table() {
        let shadow_schemas = HashSet::new();
        let target_schemas = HashSet::new();

        let mut shadow = HashMap::new();
        let target = HashMap::new();

        let mut columns = HashMap::new();
        columns.insert("id".to_string(), "INTEGER".to_string());

        shadow.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER);".to_string(),
                columns,
                column_defaults: HashMap::new(),
                constraints: Vec::new(),
            },
        );

        let diff = compute_diff(
            &shadow_schemas,
            &target_schemas,
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::CreateTable);
    }

    #[test]
    fn test_compute_diff_triggers() {
        let mut shadow_triggers = HashMap::new();
        let mut target_triggers = HashMap::new();

        shadow_triggers.insert("trg_audit".to_string(), TriggerSchema {
            schema_name: "main".to_string(),
            trigger_name: "trg_audit".to_string(),
            table_name: "users".to_string(),
            sql: "CREATE TRIGGER trg_audit AFTER INSERT ON users FOR EACH ROW INSERT INTO audit_log VALUES (NEW.id);".to_string(),
        });

        // 1. New trigger creation
        let diff1 = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &shadow_triggers,
            &target_triggers,
            &std::path::PathBuf::new(),
            &[],
        );
        assert_eq!(diff1.items.len(), 1);
        assert_eq!(diff1.items[0].item_type, DiffItemType::CreateTrigger);
        assert_eq!(diff1.items[0].obj_name, "trg_audit");

        // 2. Dropped trigger generates DROP TRIGGER trg ON tbl;
        target_triggers.insert("trg_old".to_string(), TriggerSchema {
            schema_name: "main".to_string(),
            trigger_name: "trg_old".to_string(),
            table_name: "orders".to_string(),
            sql: "CREATE TRIGGER trg_old AFTER DELETE ON orders FOR EACH ROW INSERT INTO audit_log VALUES (OLD.id);".to_string(),
        });
        let diff2 = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &target_triggers,
            &std::path::PathBuf::new(),
            &[],
        );
        assert_eq!(diff2.items.len(), 1);
        assert_eq!(diff2.items[0].item_type, DiffItemType::DropTrigger);
        assert_eq!(
            diff2.items[0].sql,
            "DROP TRIGGER \"trg_old\" ON \"orders\";"
        );
    }

    #[test]
    fn test_validate_target_catalog_and_isolation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE memory_tbl (id INT);")
            .unwrap();

        // Default target catalog
        let default_cat = validate_target_catalog(&conn, None, false).unwrap();
        assert_eq!(default_cat, "memory");

        // Specific target catalog
        let val_cat = validate_target_catalog(&conn, Some("memory"), true).unwrap();
        assert_eq!(val_cat, "memory");

        // Missing catalog error
        let err = validate_target_catalog(&conn, Some("nonexistent"), false);
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("not found on connection")
        );

        // Schema isolation test
        let schema_mem = get_schema(&conn, Some("memory")).unwrap();
        assert!(schema_mem.contains_key("memory_tbl"));

        // Attach secondary database target_db
        conn.execute_batch("ATTACH '' AS target_db; USE target_db;")
            .unwrap();
        let attached = list_attached_databases(&conn).unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].name, "target_db");

        // When target_db is active, validate_target_catalog resolves to target_db
        let default_target = validate_target_catalog(&conn, None, true).unwrap();
        assert_eq!(default_target, "target_db");

        // Explicit target_db request
        let target_res = validate_target_catalog(&conn, Some("target_db"), true).unwrap();
        assert_eq!(target_res, "target_db");
    }

    #[test]
    fn test_duckdb_v2_triggers_catalog() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE test_users (id INT, name VARCHAR);")
            .unwrap();
        // Test DuckDB native trigger syntax: statement follows FOR EACH ROW / STATEMENT directly
        conn.execute_batch("CREATE TABLE audit_log (id INT);")
            .unwrap();
        let res = conn.execute_batch("CREATE TRIGGER trg1 AFTER INSERT ON test_users FOR EACH ROW INSERT INTO audit_log VALUES (NEW.id);");
        println!("Native DuckDB trigger result: {:?}", res);
        assert!(res.is_ok());

        // Check duckdb_triggers contents
        let mut stmt = conn
            .prepare("SELECT schema_name, trigger_name, table_name, sql FROM duckdb_triggers()")
            .unwrap();
        let rows: Vec<_> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .flatten()
            .collect();
        println!("Active triggers: {:?}", rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "trg1");
        assert_eq!(rows[0].2, "test_users");

        // Test DROP TRIGGER trg ON table;
        let drop_res = conn.execute_batch("DROP TRIGGER trg1 ON test_users;");
        println!("DROP TRIGGER result: {:?}", drop_res);
        assert!(drop_res.is_ok());

        // Check duckdb_triggers columns
        if let Ok(mut stmt) = conn.prepare("DESCRIBE SELECT * FROM duckdb_triggers()") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                let cols: Vec<_> = rows.flatten().collect();
                println!("duckdb_triggers() schema: {:?}", cols);
            }
        }

        // Test Nested Schemas: create parent schema first, then child
        let res_schema = conn.execute_batch("CREATE SCHEMA finance; CREATE SCHEMA finance.reports; CREATE TABLE finance.reports.quarterly_kpis (id INT);");
        println!("Nested schema result: {:?}", res_schema);
        assert!(res_schema.is_ok());

        // Describe duckdb_schemas()
        if let Ok(mut stmt) = conn.prepare("DESCRIBE SELECT * FROM duckdb_schemas()") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            {
                let cols: Vec<_> = rows.flatten().collect();
                println!("duckdb_schemas() schema: {:?}", cols);
            }
        }
        let schema_pairs: Vec<(String, Option<String>)> = conn
            .prepare(
                "SELECT schema_name, parent_schema FROM duckdb_schemas() WHERE internal = false",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1).ok())))
            .unwrap()
            .flatten()
            .collect();
        println!("Schema / Parent pairs: {:?}", schema_pairs);

        let query_res = conn.execute_batch("SELECT * FROM finance.reports.quarterly_kpis;");
        println!("Querying finance.reports.quarterly_kpis: {:?}", query_res);
        assert!(query_res.is_ok());
    }

    #[test]
    fn test_compute_diff_add_primary_key_in_place() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "INTEGER".to_string());
        cols.insert("name".to_string(), "VARCHAR".to_string());

        target.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER, name VARCHAR);".to_string(),
                columns: cols.clone(),
                column_defaults: HashMap::new(),
                constraints: Vec::new(),
            },
        );

        shadow.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR);"
                    .to_string(),
                columns: cols,
                column_defaults: HashMap::new(),
                constraints: vec![ConstraintSchema {
                    constraint_type: "PRIMARY KEY".to_string(),
                    constraint_name: Some("users_id_pkey".to_string()),
                    columns: vec!["id".to_string()],
                    expression: None,
                    referenced_table: None,
                    referenced_columns: Vec::new(),
                }],
            },
        );

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::AddConstraint);
        assert_eq!(diff.items[0].sql, "ALTER TABLE users ADD PRIMARY KEY (id);");
        assert!(diff.items[0].selected);
    }

    #[test]
    fn test_compute_diff_add_unique_in_place() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "INTEGER".to_string());
        cols.insert("email".to_string(), "VARCHAR".to_string());

        let pk = ConstraintSchema {
            constraint_type: "PRIMARY KEY".to_string(),
            constraint_name: Some("users_id_pkey".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
            referenced_table: None,
            referenced_columns: Vec::new(),
        };

        target.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR);"
                    .to_string(),
                columns: cols.clone(),
                column_defaults: HashMap::new(),
                constraints: vec![pk.clone()],
            },
        );

        shadow.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR UNIQUE);"
                    .to_string(),
                columns: cols,
                column_defaults: HashMap::new(),
                constraints: vec![
                    pk,
                    ConstraintSchema {
                        constraint_type: "UNIQUE".to_string(),
                        constraint_name: Some("users_email_key".to_string()),
                        columns: vec!["email".to_string()],
                        expression: None,
                        referenced_table: None,
                        referenced_columns: Vec::new(),
                    },
                ],
            },
        );

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::AddConstraint);
        assert_eq!(diff.items[0].sql, "ALTER TABLE users ADD UNIQUE (email);");
        assert!(diff.items[0].selected);
    }

    #[test]
    fn test_compute_diff_add_foreign_key_recreates_table() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "INTEGER".to_string());
        cols.insert("user_id".to_string(), "INTEGER".to_string());

        let pk = ConstraintSchema {
            constraint_type: "PRIMARY KEY".to_string(),
            constraint_name: Some("orders_id_pkey".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
            referenced_table: None,
            referenced_columns: Vec::new(),
        };

        target.insert(
            "orders".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER);"
                    .to_string(),
                columns: cols.clone(),
                column_defaults: HashMap::new(),
                constraints: vec![pk.clone()],
            },
        );

        shadow.insert("orders".to_string(), TableSchema {
            create_sql: "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id));".to_string(),
            columns: cols,
            column_defaults: HashMap::new(),
            constraints: vec![
                pk,
                ConstraintSchema {
                    constraint_type: "FOREIGN KEY".to_string(),
                    constraint_name: Some("orders_user_id_fkey".to_string()),
                    columns: vec!["user_id".to_string()],
                    expression: None,
                    referenced_table: Some("users".to_string()),
                    referenced_columns: vec!["id".to_string()],
                },
            ],
        });

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::RecreateTable);
        assert!(diff.items[0].sql.contains("BEGIN TRANSACTION;"));
        assert!(diff.items[0].sql.contains("DROP TABLE \"orders\";"));
        assert!(
            diff.items[0]
                .sql
                .contains("ALTER TABLE \"orders__duckpack_swap\" RENAME TO \"orders\";")
        );
        assert!(diff.items[0].sql.contains("COMMIT;"));
    }

    #[test]
    fn test_compute_diff_drop_constraint_recreates_table() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "INTEGER".to_string());
        cols.insert("email".to_string(), "VARCHAR".to_string());

        let pk = ConstraintSchema {
            constraint_type: "PRIMARY KEY".to_string(),
            constraint_name: Some("users_id_pkey".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
            referenced_table: None,
            referenced_columns: Vec::new(),
        };
        let uq = ConstraintSchema {
            constraint_type: "UNIQUE".to_string(),
            constraint_name: Some("users_email_key".to_string()),
            columns: vec!["email".to_string()],
            expression: None,
            referenced_table: None,
            referenced_columns: Vec::new(),
        };

        target.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR UNIQUE);"
                    .to_string(),
                columns: cols.clone(),
                column_defaults: HashMap::new(),
                constraints: vec![pk.clone(), uq],
            },
        );

        // Shadow removed the UNIQUE constraint
        shadow.insert(
            "users".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR);"
                    .to_string(),
                columns: cols,
                column_defaults: HashMap::new(),
                constraints: vec![pk],
            },
        );

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::RecreateTable);
    }

    #[test]
    fn test_get_schema_duckdb_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE users (id INT PRIMARY KEY, email VARCHAR UNIQUE);")
            .unwrap();
        let schema = get_schema(&conn, Some("memory")).unwrap();
        assert!(schema.contains_key("users"));
        let users_tbl = &schema["users"];
        assert_eq!(users_tbl.columns.len(), 2);
        assert_eq!(users_tbl.constraints.len(), 2);
        assert!(
            users_tbl
                .constraints
                .iter()
                .any(|c| c.constraint_type == "PRIMARY KEY" && c.columns == vec!["id"])
        );
        assert!(
            users_tbl
                .constraints
                .iter()
                .any(|c| c.constraint_type == "UNIQUE" && c.columns == vec!["email"])
        );
    }

    #[test]
    fn test_compute_diff_column_defaults() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut cols = HashMap::new();
        cols.insert("id".to_string(), "INTEGER".to_string());
        cols.insert("processed".to_string(), "BOOLEAN".to_string());
        cols.insert("col_drop_def".to_string(), "VARCHAR".to_string());
        cols.insert("col_change_def".to_string(), "INTEGER".to_string());

        let mut target_defs = HashMap::new();
        target_defs.insert("col_drop_def".to_string(), "'old_default'".to_string());
        target_defs.insert("col_change_def".to_string(), "1".to_string());

        let mut shadow_defs = HashMap::new();
        shadow_defs.insert("processed".to_string(), "false".to_string());
        shadow_defs.insert("col_change_def".to_string(), "2".to_string());

        target.insert("orders".to_string(), TableSchema {
            create_sql: "CREATE TABLE orders (id INTEGER, processed BOOLEAN, col_drop_def VARCHAR, col_change_def INTEGER);".to_string(),
            columns: cols.clone(),
            column_defaults: target_defs,
            constraints: Vec::new(),
        });

        shadow.insert("orders".to_string(), TableSchema {
            create_sql: "CREATE TABLE orders (id INTEGER, processed BOOLEAN DEFAULT false, col_drop_def VARCHAR, col_change_def INTEGER DEFAULT 2);".to_string(),
            columns: cols,
            column_defaults: shadow_defs,
            constraints: Vec::new(),
        });

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::AlterTable);
        assert!(
            diff.items[0]
                .sql
                .contains("ALTER TABLE orders ALTER processed SET DEFAULT false;")
        );
        assert!(
            diff.items[0]
                .sql
                .contains("ALTER TABLE orders ALTER col_change_def SET DEFAULT 2;")
        );
        assert!(
            diff.items[0]
                .sql
                .contains("ALTER TABLE orders ALTER col_drop_def DROP DEFAULT;")
        );
    }

    #[test]
    fn test_compute_diff_add_column_with_default() {
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut target_cols = HashMap::new();
        target_cols.insert("id".to_string(), "INTEGER".to_string());

        let mut shadow_cols = target_cols.clone();
        shadow_cols.insert("processed".to_string(), "BOOLEAN".to_string());

        let mut shadow_defs = HashMap::new();
        shadow_defs.insert("processed".to_string(), "false".to_string());

        target.insert(
            "orders".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE orders (id INTEGER);".to_string(),
                columns: target_cols,
                column_defaults: HashMap::new(),
                constraints: Vec::new(),
            },
        );

        shadow.insert(
            "orders".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE orders (id INTEGER, processed BOOLEAN DEFAULT false);"
                    .to_string(),
                columns: shadow_cols,
                column_defaults: shadow_defs,
                constraints: Vec::new(),
            },
        );

        let diff = compute_diff(
            &HashSet::new(),
            &HashSet::new(),
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::AlterTable);
        assert_eq!(
            diff.items[0].sql,
            "ALTER TABLE orders ADD COLUMN processed BOOLEAN DEFAULT false;"
        );
    }

    #[test]
    fn test_nested_schemas_introspection_and_ordering() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE SCHEMA region; \
             CREATE SCHEMA region.sales; \
             CREATE SCHEMA region.sales.apac; \
             CREATE TABLE region.sales.orders (revenue DECIMAL); \
             CREATE TABLE region.sales.apac.targets (goal INT);",
        )
        .unwrap();

        let schemas = get_schemas(&conn, None).unwrap();
        assert!(schemas.contains("region"));
        assert!(schemas.contains("region.sales"));
        assert!(schemas.contains("region.sales.apac"));

        let tables = get_schema(&conn, None).unwrap();
        assert!(tables.contains_key("region.sales.orders"));
        assert!(tables.contains_key("region.sales.apac.targets"));

        // Diff creation against empty target
        let empty_target_schemas = HashSet::new();
        let diff = compute_diff(
            &schemas,
            &empty_target_schemas,
            &tables,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        let schema_items: Vec<_> = diff
            .items
            .iter()
            .filter(|i| i.item_type == DiffItemType::CreateSchema)
            .collect();
        assert_eq!(schema_items.len(), 3);
        // Assert topological order: root schema first, then level 1, then level 2
        assert_eq!(schema_items[0].obj_name, "region");
        assert_eq!(schema_items[0].sql, "CREATE SCHEMA \"region\";");
        assert_eq!(schema_items[1].obj_name, "region.sales");
        assert_eq!(schema_items[1].sql, "CREATE SCHEMA \"region\".\"sales\";");
        assert_eq!(schema_items[2].obj_name, "region.sales.apac");
        assert_eq!(
            schema_items[2].sql,
            "CREATE SCHEMA \"region\".\"sales\".\"apac\";"
        );

        // Diff dropping against empty shadow (target has all 3 schemas)
        let empty_shadow_schemas = HashSet::new();
        let drop_diff = compute_diff(
            &empty_shadow_schemas,
            &schemas,
            &HashMap::new(),
            &tables,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        let drop_items: Vec<_> = drop_diff
            .items
            .iter()
            .filter(|i| i.item_type == DiffItemType::DropSchema)
            .collect();
        assert_eq!(drop_items.len(), 3);
        // Assert reverse topological order for drops: child schema dropped before parent
        assert_eq!(drop_items[0].obj_name, "region.sales.apac");
        assert_eq!(
            drop_items[0].sql,
            "DROP SCHEMA \"region\".\"sales\".\"apac\" CASCADE;"
        );
        assert_eq!(drop_items[1].obj_name, "region.sales");
        assert_eq!(
            drop_items[1].sql,
            "DROP SCHEMA \"region\".\"sales\" CASCADE;"
        );
        assert_eq!(drop_items[2].obj_name, "region");
        assert_eq!(drop_items[2].sql, "DROP SCHEMA \"region\" CASCADE;");
    }

    #[test]
    fn test_nested_schemas_apply_diff() {
        let conn = Connection::open_in_memory().unwrap();
        // Prepare diff containing nested schema and table
        let mut diff = DiffResult { items: Vec::new() };
        diff.items.push(DiffItem {
            item_type: DiffItemType::CreateSchema,
            obj_name: "region".to_string(),
            sql: "CREATE SCHEMA \"region\";".to_string(),
            old_def: None,
            new_def: None,
            selected: true,
            locked: false,
            ignored: false,
        });
        diff.items.push(DiffItem {
            item_type: DiffItemType::CreateSchema,
            obj_name: "region.sales".to_string(),
            sql: "CREATE SCHEMA \"region\".\"sales\";".to_string(),
            old_def: None,
            new_def: None,
            selected: true,
            locked: false,
            ignored: false,
        });
        diff.items.push(DiffItem {
            item_type: DiffItemType::CreateTable,
            obj_name: "region.sales.orders".to_string(),
            sql: "CREATE TABLE region.sales.orders (revenue DECIMAL);".to_string(),
            old_def: None,
            new_def: None,
            selected: true,
            locked: false,
            ignored: false,
        });

        let proj_dir = std::path::PathBuf::from("/tmp");
        let apply_res = crate::engine::apply_diff(&conn, &diff, false, false, &proj_dir, None);
        assert!(apply_res.is_ok(), "Apply failed: {:?}", apply_res.err());

        // Verify schemas exist
        let schemas = get_schemas(&conn, None).unwrap();
        assert!(schemas.contains("region"));
        assert!(schemas.contains("region.sales"));

        // Verify table exists and can be queried
        let query_res = conn.execute_batch("SELECT revenue FROM region.sales.orders;");
        assert!(query_res.is_ok());
    }

    #[test]
    fn test_qualified_identifier_parsing_and_quoting() {
        let q1 = QualifiedIdentifier::parse("users");
        assert_eq!(q1.leaf_name(), "users");
        assert_eq!(q1.schema_prefix(), "");
        assert_eq!(q1.full_schema(), None);
        assert_eq!(q1.quoted_qualified_name(), "\"users\"");

        let q2 = QualifiedIdentifier::parse("finance.reports.q3");
        assert_eq!(q2.leaf_name(), "q3");
        assert_eq!(q2.schema_prefix(), "finance.reports.");
        assert_eq!(q2.full_schema(), Some("finance.reports".to_string()));
        assert_eq!(q2.quoted_qualified_name(), "\"finance\".\"reports\".\"q3\"");
    }

    #[test]
    fn test_secure_view_diff_replacement() {
        let shadow_schemas = HashSet::new();
        let target_schemas = HashSet::new();
        let mut shadow_views = HashMap::new();
        let mut target_views = HashMap::new();

        shadow_views.insert(
            "v_sec".to_string(),
            "CREATE SECURE VIEW v_sec AS SELECT 1;".to_string(),
        );
        target_views.insert(
            "v_sec".to_string(),
            "CREATE SECURE VIEW v_sec AS SELECT 0;".to_string(),
        );

        let diff = compute_diff(
            &shadow_schemas,
            &target_schemas,
            &HashMap::new(),
            &HashMap::new(),
            &shadow_views,
            &target_views,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &std::path::PathBuf::new(),
            &[],
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::CreateView);
        assert_eq!(
            diff.items[0].sql,
            "CREATE OR REPLACE SECURE VIEW v_sec AS SELECT 1;"
        );
    }

    #[test]
    fn test_recreate_table_trigger_preservation() {
        let shadow_schemas = HashSet::new();
        let target_schemas = HashSet::new();
        let mut shadow = HashMap::new();
        let mut target = HashMap::new();

        let mut shadow_cols = HashMap::new();
        shadow_cols.insert("id".to_string(), "INTEGER".to_string());
        let mut target_cols = HashMap::new();
        target_cols.insert("id".to_string(), "INTEGER".to_string());

        // Target has no check constraint, shadow adds a check constraint requiring recreate
        let shadow_constr = vec![ConstraintSchema {
            constraint_type: "CHECK".to_string(),
            constraint_name: Some("chk_id".to_string()),
            columns: vec!["id".to_string()],
            expression: Some("id > 0".to_string()),
            referenced_table: None,
            referenced_columns: vec![],
        }];

        shadow.insert(
            "finance.reports.kpis".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE finance.reports.kpis (id INTEGER CHECK (id > 0));"
                    .to_string(),
                columns: shadow_cols,
                column_defaults: HashMap::new(),
                constraints: shadow_constr,
            },
        );
        target.insert(
            "finance.reports.kpis".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE finance.reports.kpis (id INTEGER);".to_string(),
                columns: target_cols,
                column_defaults: HashMap::new(),
                constraints: vec![],
            },
        );

        let mut shadow_triggers = HashMap::new();
        let mut target_triggers = HashMap::new();

        shadow_triggers.insert("trg_kpi".to_string(), TriggerSchema {
            schema_name: "finance.reports".to_string(),
            trigger_name: "trg_kpi".to_string(),
            table_name: "finance.reports.kpis".to_string(),
            sql: "CREATE TRIGGER trg_kpi AFTER INSERT ON finance.reports.kpis FOR EACH ROW INSERT INTO audit VALUES (NEW.id);".to_string(),
        });
        target_triggers.insert("trg_kpi".to_string(), TriggerSchema {
            schema_name: "finance.reports".to_string(),
            trigger_name: "trg_kpi".to_string(),
            table_name: "finance.reports.kpis".to_string(),
            sql: "CREATE TRIGGER trg_kpi AFTER INSERT ON finance.reports.kpis FOR EACH ROW INSERT INTO audit VALUES (NEW.id);".to_string(),
        });

        let diff = compute_diff(
            &shadow_schemas,
            &target_schemas,
            &shadow,
            &target,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &shadow_triggers,
            &target_triggers,
            &std::path::PathBuf::new(),
            &[],
        );

        // Expect RecreateTable AND CreateTrigger
        let recreate_item = diff
            .items
            .iter()
            .find(|i| i.item_type == DiffItemType::RecreateTable);
        assert!(recreate_item.is_some(), "RecreateTable should be present");
        let recreate_sql = &recreate_item.unwrap().sql;
        // Verify 3-level table swap generates leaf rename TO "kpis"
        assert!(recreate_sql.contains(
            "ALTER TABLE \"finance\".\"reports\".\"kpis__duckpack_swap\" RENAME TO \"kpis\";"
        ));

        let trg_item = diff
            .items
            .iter()
            .find(|i| i.item_type == DiffItemType::CreateTrigger);
        assert!(
            trg_item.is_some(),
            "CreateTrigger should be auto-emitted for recreated table"
        );
    }

    #[test]
    fn test_catalog_snapshot_diff_project_and_db_executor_seam() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_snapshot_seam_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(temp_dir.join("schemas")).unwrap();
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();
        std::fs::create_dir_all(temp_dir.join("triggers")).unwrap();

        std::fs::write(
            temp_dir.join("schemas").join("core.sql"),
            "CREATE SCHEMA IF NOT EXISTS core;",
        )
        .unwrap();
        std::fs::write(
            temp_dir.join("tables").join("events.sql"),
            "CREATE TABLE core.events (id INTEGER PRIMARY KEY, label VARCHAR DEFAULT 'new');\nCREATE TABLE core.event_log (event_id INTEGER);",
        )
        .unwrap();
        std::fs::write(
            temp_dir.join("triggers").join("trg_events.sql"),
            "CREATE TRIGGER trg_events AFTER INSERT ON core.events FOR EACH ROW INSERT INTO core.event_log VALUES (NEW.id);",
        )
        .unwrap();

        let mut target_conn = Connection::open_in_memory().unwrap();
        target_conn
            .execute_batch("CREATE TABLE legacy_tbl (id INTEGER);")
            .unwrap();

        // 1. Test diff_project across DbExecutor seam
        let active_cat = validate_target_catalog_exec(&mut target_conn, None, true).unwrap();
        let diff_res = diff_project(&temp_dir, &mut target_conn, Some(&active_cat)).unwrap();

        assert!(
            diff_res
                .items
                .iter()
                .any(|i| i.item_type == DiffItemType::CreateSchema && i.obj_name == "core")
        );
        assert!(
            diff_res
                .items
                .iter()
                .any(|i| i.item_type == DiffItemType::CreateTable && i.obj_name == "core.events")
        );
        assert!(
            diff_res
                .items
                .iter()
                .any(|i| i.item_type == DiffItemType::CreateTrigger
                    && i.obj_name.ends_with("trg_events"))
        );
        let drop_item = diff_res
            .items
            .iter()
            .find(|i| i.item_type == DiffItemType::DropTable && i.obj_name == "legacy_tbl");
        assert!(drop_item.is_some());
        assert!(drop_item.unwrap().is_destructive());

        // 2. Test pull_object_to_project from target CatalogSnapshot
        let target_snap = CatalogSnapshot::introspect(&mut target_conn, Some(&active_cat)).unwrap();
        let pulled_path = target_snap
            .pull_object_to_project(&temp_dir, &DiffItemType::DropTable, "legacy_tbl")
            .unwrap();
        assert!(pulled_path.exists());

        // 3. Re-diff after pull: legacy_tbl should no longer be marked for drop
        let diff_after_pull = diff_project(&temp_dir, &mut target_conn, Some(&active_cat)).unwrap();
        assert!(
            !diff_after_pull
                .items
                .iter()
                .any(|i| i.obj_name == "legacy_tbl")
        );

        // 4. Apply via apply_diff_with_executor and verify zero remaining diff
        crate::engine::apply_diff_with_executor(
            &mut target_conn,
            &diff_after_pull,
            false,
            false,
            &temp_dir,
            Some(&active_cat),
        )
        .unwrap();

        let diff_final = diff_project(&temp_dir, &mut target_conn, Some(&active_cat)).unwrap();
        assert!(
            diff_final.is_empty(),
            "Expected 0 remaining changes, got: {:?}",
            diff_final.items
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_pull_object_to_project_rejects_path_traversal() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_pull_traversal_test_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut snap = CatalogSnapshot::default();
        snap.tables.insert(
            "../../evil".to_string(),
            TableSchema {
                create_sql: "CREATE TABLE \"../../evil\" (id INT);".to_string(),
                columns: HashMap::new(),
                column_defaults: HashMap::new(),
                constraints: Vec::new(),
            },
        );
        snap.views.insert("sub/view".to_string(), "CREATE VIEW \"sub/view\" AS SELECT 1;".to_string());

        assert!(snap.pull_object_to_project(&temp_dir, &DiffItemType::DropTable, "../../evil").is_err());
        assert!(snap.pull_object_to_project(&temp_dir, &DiffItemType::DropView, "sub/view").is_err());
        assert!(validate_safe_project_filename("..").is_err());
        assert!(validate_safe_project_filename("a\\b").is_err());
        assert!(validate_safe_project_filename("valid_schema.valid_table").is_ok());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
