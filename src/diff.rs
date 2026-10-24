use anyhow::Result;
use duckdb::Connection;
use std::collections::HashMap;
use std::path::Path;
use regex::Regex;
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
        if !self.constraint_type.eq_ignore_ascii_case(&other.constraint_type) {
            return false;
        }
        let self_cols: Vec<String> = self.columns.iter().map(|c| c.to_lowercase()).collect();
        let other_cols: Vec<String> = other.columns.iter().map(|c| c.to_lowercase()).collect();
        if self_cols != other_cols {
            return false;
        }
        match self.constraint_type.to_uppercase().as_str() {
            "CHECK" => {
                let self_expr = self.expression.as_deref().unwrap_or("").trim().trim_matches(|c| c == '(' || c == ')').trim();
                let other_expr = other.expression.as_deref().unwrap_or("").trim().trim_matches(|c| c == '(' || c == ')').trim();
                self_expr.eq_ignore_ascii_case(other_expr)
            }
            "FOREIGN KEY" => {
                let self_tbl = self.referenced_table.as_deref().unwrap_or("").to_lowercase();
                let other_tbl = other.referenced_table.as_deref().unwrap_or("").to_lowercase();
                let self_simple = self_tbl.split('.').last().unwrap_or(&self_tbl);
                let other_simple = other_tbl.split('.').last().unwrap_or(&other_tbl);
                if self_simple != other_simple {
                    return false;
                }
                let self_ref_cols: Vec<String> = self.referenced_columns.iter().map(|c| c.to_lowercase()).collect();
                let other_ref_cols: Vec<String> = other.referenced_columns.iter().map(|c| c.to_lowercase()).collect();
                self_ref_cols == other_ref_cols
            }
            _ => true,
        }
    }
}

/// Represents the resolved structure of a DuckDB table.
/// Contains the raw creation SQL, a map of column names to their data types, and parsed constraints.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub create_sql: String,
    pub columns: HashMap<String, String>,
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

/// Represents an individual trigger in a DuckDB catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSchema {
    pub schema_name: String,
    pub trigger_name: String,
    pub table_name: String,
    pub sql: String,
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

pub fn list_attached_databases(conn: &Connection) -> Result<Vec<AttachedDatabase>> {
    let is_quack = conn
        .query_row(
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    if is_quack {
        let quack_db_sql = "SELECT database_name, type, path, readonly FROM target_db.query('SELECT database_name, type, path, readonly FROM duckdb_databases() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_db_sql) {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok(AttachedDatabase {
                    name: r.get(0)?,
                    db_type: r.get(1)?,
                    path: r.get(2).ok(),
                    readonly: r.get(3).unwrap_or(false),
                })
            }) {
                let mut remote_dbs = Vec::new();
                for row in rows.flatten() {
                    remote_dbs.push(row);
                }
                if !remote_dbs.is_empty() {
                    return Ok(remote_dbs);
                }
            }
        }
    }

    let mut stmt = conn.prepare("SELECT database_name, type, path, readonly FROM duckdb_databases() WHERE internal = false")?;
    let rows = stmt.query_map([], |r| {
        Ok(AttachedDatabase {
            name: r.get(0)?,
            db_type: r.get(1)?,
            path: r.get(2).ok(),
            readonly: r.get(3).unwrap_or(false),
        })
    })?;
    let mut dbs = Vec::new();
    for row in rows {
        dbs.push(row?);
    }
    // If other databases are attached to this connection (e.g. target_db or secondary catalogs),
    // and target_db represents an attached local file, filter out the client's internal default "memory" harness.
    if dbs.len() > 1 && dbs.iter().any(|d| d.name == "target_db") {
        dbs.retain(|d| d.name != "memory");
    }
    Ok(dbs)
}

pub fn validate_target_catalog(conn: &Connection, catalog: Option<&str>, for_apply: bool) -> Result<String> {
    let attached = list_attached_databases(conn).unwrap_or_default();
    let is_quack = conn
        .query_row(
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    let target = match catalog {
        Some(c) => c.to_string(),
        None => {
            if is_quack {
                conn.query_row("SELECT * FROM target_db.query('SELECT current_database()')", [], |r| r.get(0))
                    .unwrap_or_else(|_| "memory".to_string())
            } else {
                conn.query_row("SELECT current_database()", [], |r| r.get(0))
                    .unwrap_or_else(|_| "memory".to_string())
            }
        }
    };

    if let Some(db_info) = attached.iter().find(|d| d.name.eq_ignore_ascii_case(&target)) {
        if for_apply {
            if db_info.readonly {
                anyhow::bail!("Cannot apply migrations to database '{}': attached in READ_ONLY mode.", db_info.name);
            }
            let typ = db_info.db_type.to_lowercase();
            if typ != "duckdb" && typ != "quack" {
                anyhow::bail!(
                    "Cannot apply declarative schema migrations to '{}' (engine type: {}). \
                    DuckPack declarative schema management is designed specifically for DuckDB catalogs. \
                    External attached engines are supported for querying and DML, but schema DDL is managed externally.",
                    db_info.name, db_info.db_type
                );
            }
        }
        Ok(db_info.name.clone())
    } else if attached.is_empty() {
        Ok(target)
    } else {
        let available: Vec<_> = attached.iter().map(|d| d.name.as_str()).collect();
        anyhow::bail!("Attached database '{}' not found on connection. Available databases: {:?}", target, available);
    }
}

/// Extracts the entire table schema from a DuckDB connection for a specific catalog.
/// It queries `duckdb_tables()` for table definitions and `duckdb_columns()` for specific datatypes.
/// Returns a HashMap mapping `schema.table` -> `TableSchema`.
pub fn get_schema(conn: &Connection, catalog: Option<&str>) -> Result<HashMap<String, TableSchema>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_else(|_| "memory".to_string()),
    };
    let safe_cat = target_cat.replace('\'', "''");

    let sql = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
         COALESCE(sql, '/* Remote table: ' || table_name || ' */') \
         FROM duckdb_tables() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut tables: HashMap<String, TableSchema> = HashMap::new();
    
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (table_name, sql) = row?;
        tables.insert(table_name.clone(), TableSchema {
            create_sql: sql,
            columns: HashMap::new(),
            constraints: Vec::new(),
        });
    }

    let is_quack = conn
        .query_row(
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    if is_quack || tables.is_empty() {
        let q_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             COALESCE(sql, '/* Remote table: ' || table_name || ' */') \
             FROM target_db.query('SELECT schema_name, table_name, sql FROM duckdb_tables() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database())')",
            safe_cat
        );
        if let Ok(mut q_stmt) = conn.prepare(&q_sql) {
            if let Ok(rows) = q_stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
                for row in rows.flatten() {
                    let (table_name, sql) = row;
                    tables.entry(table_name).or_insert_with(|| TableSchema {
                        create_sql: sql.clone(),
                        columns: HashMap::new(),
                        constraints: Vec::new(),
                    });
                }
            }
        }
    }

    let col_sql = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
         column_name, data_type \
         FROM duckdb_columns() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let mut col_stmt = conn.prepare(&col_sql)?;
    let col_rows = col_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;

    for row in col_rows {
        let (table_name, column_name, data_type) = row?;
        if let Some(table) = tables.get_mut(&table_name) {
            table.columns.insert(column_name, data_type);
        }
    }

    let any_col_empty = tables.values().any(|t| t.columns.is_empty());
    if is_quack || any_col_empty {
        let q_col_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             column_name, data_type \
             FROM target_db.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false AND (database_name = ''{}'' OR database_name = current_database())')",
            safe_cat
        );
        if let Ok(mut q_col_stmt) = conn.prepare(&q_col_sql) {
            if let Ok(q_col_rows) = q_col_stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))) {
                for row in q_col_rows.flatten() {
                    let (table_name, column_name, data_type) = row;
                    if let Some(table) = tables.get_mut(&table_name) {
                        table.columns.insert(column_name, data_type);
                    }
                }
            }
        }
    }


    // Introspect table constraints (PRIMARY KEY, UNIQUE, FOREIGN KEY, CHECK)
    let constr_sql = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
         constraint_type, \
         constraint_name, \
         COALESCE(array_to_string(constraint_column_names, ','), ''), \
         expression, \
         referenced_table, \
         COALESCE(array_to_string(referenced_column_names, ','), '') \
         FROM duckdb_constraints() \
         WHERE (database_name = '{}' OR database_name = 'target_db') AND constraint_type IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY', 'CHECK')",
        safe_cat
    );
    if let Ok(mut constr_stmt) = conn.prepare(&constr_sql) {
        if let Ok(constr_rows) = constr_stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        }) {
            for row in constr_rows.flatten() {
                let (table_name, c_type, c_name, cols_str, expr, ref_tbl, ref_cols_str) = row;
                if let Some(table) = tables.get_mut(&table_name) {
                    let cols: Vec<String> = if cols_str.is_empty() {
                        Vec::new()
                    } else {
                        cols_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                    };
                    let ref_cols: Vec<String> = if ref_cols_str.is_empty() {
                        Vec::new()
                    } else {
                        ref_cols_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                    };
                    table.constraints.push(ConstraintSchema {
                        constraint_type: c_type,
                        constraint_name: c_name,
                        columns: cols,
                        expression: expr,
                        referenced_table: ref_tbl,
                        referenced_columns: ref_cols,
                    });
                }
            }
        }
    }

    let is_quack = conn
        .query_row(
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    if is_quack {
        let q_constr_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, \
             constraint_type, \
             constraint_name, \
             COALESCE(constraint_column_names, ''), \
             expression, \
             referenced_table, \
             COALESCE(referenced_column_names, '') \
             FROM target_db.query('SELECT schema_name, table_name, constraint_type, constraint_name, array_to_string(constraint_column_names, '','') as constraint_column_names, expression, referenced_table, array_to_string(referenced_column_names, '','') as referenced_column_names FROM duckdb_constraints() WHERE constraint_type IN (''PRIMARY KEY'', ''UNIQUE'', ''FOREIGN KEY'', ''CHECK'') AND (database_name = ''{}'' OR database_name = current_database())')",
            safe_cat
        );
        if let Ok(mut q_stmt) = conn.prepare(&q_constr_sql) {
            if let Ok(q_rows) = q_stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            }) {
                for row in q_rows.flatten() {
                    let (table_name, c_type, c_name, cols_str, expr, ref_tbl, ref_cols_str) = row;
                    if let Some(table) = tables.get_mut(&table_name) {
                        let cols: Vec<String> = if cols_str.is_empty() {
                            Vec::new()
                        } else {
                            cols_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                        };
                        let ref_cols: Vec<String> = if ref_cols_str.is_empty() {
                            Vec::new()
                        } else {
                            ref_cols_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
                        };
                        let new_c = ConstraintSchema {
                            constraint_type: c_type,
                            constraint_name: c_name,
                            columns: cols,
                            expression: expr,
                            referenced_table: ref_tbl,
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
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_else(|_| "memory".to_string()),
    };
    let safe_cat = target_cat.replace('\'', "''");

    let sql = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, \
         COALESCE(sql, '/* Remote view: ' || view_name || ' */') \
         FROM duckdb_views() WHERE internal = false AND database_name = '{}'",
        safe_cat
    );
    let mut stmt = conn.prepare(&sql)?;
    
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut views = HashMap::new();
    for row in rows {
        let (name, sql) = row?;
        views.insert(name, sql);
    }
    if views.is_empty() {
        let q_sql = format!(
            "SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, \
             COALESCE(sql, '/* Remote view: ' || view_name || ' */') \
             FROM target_db.query('SELECT schema_name, view_name, sql FROM duckdb_views() WHERE internal = false AND database_name = ''{}''')",
            safe_cat
        );
        if let Ok(mut q_stmt) = conn.prepare(&q_sql) {
            if let Ok(rows) = q_stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
                for row in rows.flatten() {
                    let (name, sql) = row;
                    views.insert(name, sql);
                }
            }
        }
    }
    Ok(views)
}

/// Extracts all macro (function) definitions from a DuckDB connection for a specific catalog.
pub fn get_macros(conn: &Connection, catalog: Option<&str>) -> Result<HashMap<String, String>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_else(|_| "memory".to_string()),
    };
    let safe_cat = target_cat.replace('\'', "''");

    let sql = format!(
        "SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END, \
         macro_definition, CAST(parameters AS VARCHAR) \
         FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false AND database_name = '{}'",
        safe_cat
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
    let mut macros = HashMap::new();
    for row in rows {
        let (name, def, params) = row?;
        let params_clean = params.trim_matches(|c| c == '[' || c == ']');
        let sql = format!("CREATE MACRO {}({}) AS {};", name, params_clean, def);
        macros.insert(name, sql);
    }
    Ok(macros)
}

/// Extracts all schema names (excluding system schemas) from a DuckDB connection for a specific catalog.
pub fn get_schemas(conn: &Connection, catalog: Option<&str>) -> Result<std::collections::HashSet<String>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_else(|_| "memory".to_string()),
    };
    let safe_cat = target_cat.replace('\'', "''");

    let sql = format!(
        "SELECT schema_name FROM duckdb_schemas() WHERE internal = false AND database_name = '{}' AND schema_name NOT IN ('information_schema', 'pg_catalog', 'main')",
        safe_cat
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| Ok(row.get::<_, String>(0)?))?;
    let mut schemas = std::collections::HashSet::new();
    for row in rows {
        schemas.insert(row?);
    }
    if schemas.is_empty() {
        let q_sql = format!(
            "SELECT schema_name FROM target_db.query('SELECT schema_name FROM duckdb_schemas() WHERE internal = false AND database_name = ''{}'' AND schema_name NOT IN (''information_schema'', ''pg_catalog'', ''main'')')",
            safe_cat
        );
        if let Ok(mut q_stmt) = conn.prepare(&q_sql) {
            if let Ok(rows) = q_stmt.query_map([], |row| Ok(row.get::<_, String>(0)?)) {
                for row in rows.flatten() {
                    schemas.insert(row);
                }
            }
        }
    }
    Ok(schemas)
}

/// Extracts all trigger definitions from a DuckDB connection for a specific catalog.
pub fn get_triggers(conn: &Connection, catalog: Option<&str>) -> Result<HashMap<String, TriggerSchema>> {
    let target_cat = match catalog {
        Some(c) => c.to_string(),
        None => conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_else(|_| "memory".to_string()),
    };
    let safe_cat = target_cat.replace('\'', "''");
    let mut triggers = HashMap::new();

    let sql = format!(
        "SELECT schema_name, trigger_name, table_name, COALESCE(sql, '') \
         FROM duckdb_triggers() WHERE database_name = '{}'",
        safe_cat
    );
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            for row in rows.flatten() {
                let (schema, trg_name, table, trg_sql) = row;
                let key = if schema == "main" {
                    trg_name.clone()
                } else {
                    format!("{}.{}", schema, trg_name)
                };
                triggers.insert(key, TriggerSchema {
                    schema_name: schema,
                    trigger_name: trg_name,
                    table_name: table,
                    sql: trg_sql,
                });
            }
        }
    }

    if triggers.is_empty() {
        let q_sql = format!(
            "SELECT schema_name, trigger_name, table_name, COALESCE(sql, '') \
             FROM target_db.query('SELECT schema_name, trigger_name, table_name, sql FROM duckdb_triggers() WHERE database_name = ''{}''')",
            safe_cat
        );
        if let Ok(mut q_stmt) = conn.prepare(&q_sql) {
            if let Ok(rows) = q_stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            }) {
                for row in rows.flatten() {
                    let (schema, trg_name, table, trg_sql) = row;
                    let key = if schema == "main" {
                        trg_name.clone()
                    } else {
                        format!("{}.{}", schema, trg_name)
                    };
                    triggers.insert(key, TriggerSchema {
                        schema_name: schema,
                        trigger_name: trg_name,
                        table_name: table,
                        sql: trg_sql,
                    });
                }
            }
        }
    }

    Ok(triggers)
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
    ignore_list: &[String]
) -> DiffResult {
    let mut diff = DiffResult {
        items: Vec::new(),
    };

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
    for schema_name in shadow_schemas {
        if is_ignored(schema_name) { continue; }
        if !target_schemas.contains(schema_name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateSchema,
                obj_name: schema_name.clone(),
                sql: format!("CREATE SCHEMA \"{}\";", schema_name),
                old_def: None,
                new_def: None,
                selected: true,
                locked: false,
                ignored: false,
            });
        }
    }
    
    for schema_name in target_schemas {
        if is_ignored(schema_name) { continue; }
        if !shadow_schemas.contains(schema_name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropSchema,
                obj_name: schema_name.clone(),
                sql: format!("DROP SCHEMA \"{}\" CASCADE;", schema_name),
                old_def: None,
                new_def: None,
                selected: true,
                locked: false,
                ignored: false,
            });
        }
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
                    if matched_created_cols.contains(created) { continue; }
                    if strsim::jaro_winkler(dropped, created) > 0.8 {
                        matched_dropped_cols.insert(dropped.clone());
                        matched_created_cols.insert(created.clone());
                        safe_sqls.push(format!("ALTER TABLE {} RENAME COLUMN {} TO {};", table_name, dropped, created));
                        break;
                    }
                }
            }

            for col_name in &created_cols {
                if !matched_created_cols.contains(col_name) {
                    let shadow_type = shadow_table.columns.get(col_name).unwrap();
                    safe_sqls.push(format!("ALTER TABLE {} ADD COLUMN {} {};", table_name, col_name, shadow_type));
                }
            }

            for col_name in &dropped_cols {
                if !matched_dropped_cols.contains(col_name) {
                    destructive_sqls.push(format!("ALTER TABLE {} DROP COLUMN {};", table_name, col_name));
                }
            }

            for (col_name, shadow_type) in shadow_table.columns.iter() {
                if let Some(target_type) = target_table.columns.get(col_name) {
                    if shadow_type != target_type {
                        safe_sqls.push(format!("ALTER TABLE {} ALTER {} TYPE {};", table_name, col_name, shadow_type));
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
                        let target_has_pk = target_table.constraints.iter().any(|c| c.constraint_type.eq_ignore_ascii_case("PRIMARY KEY"));
                        if target_has_pk {
                            needs_recreate = true;
                        } else {
                            in_place_constraints.push(format!("ALTER TABLE {} ADD PRIMARY KEY ({});", table_name, ac.columns.join(", ")));
                        }
                    }
                    "UNIQUE" => {
                        in_place_constraints.push(format!("ALTER TABLE {} ADD UNIQUE ({});", table_name, ac.columns.join(", ")));
                    }
                    "FOREIGN KEY" | "CHECK" => {
                        needs_recreate = true;
                    }
                    _ => {}
                }
            }

            if needs_recreate {
                let (schema_prefix, simple_name) = match table_name.split_once('.') {
                    Some((s, t)) => (format!("{}.", s), t),
                    None => (String::new(), table_name.as_str()),
                };
                let swap_table_name = format!("{}{}__duckpack_swap", schema_prefix, simple_name);

                let create_table_re = Regex::new(r"(?i)^\s*CREATE\s+TABLE\s+[^\s(]+").unwrap();
                let swap_create_sql = create_table_re.replace(&shadow_table.create_sql, format!("CREATE TABLE {}", swap_table_name));

                let mut common_cols: Vec<String> = shadow_table.columns.keys()
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
                    let quoted_cols: Vec<String> = common_cols.iter().map(|c| format!("\"{}\"", c)).collect();
                    let cols_str = quoted_cols.join(", ");
                    swap_script.push_str(&format!(
                        "INSERT INTO {} ({}) SELECT {} FROM {};\n",
                        swap_table_name, cols_str, cols_str, table_name
                    ));
                }

                swap_script.push_str(&format!("DROP TABLE {};\n", table_name));
                swap_script.push_str(&format!("ALTER TABLE {} RENAME TO {};\n", swap_table_name, simple_name));
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
            } else {
                if !safe_sqls.is_empty() {
                    let has_rename = safe_sqls.iter().any(|s| s.contains("RENAME COLUMN"));
                    let ignored = is_ignored(table_name);
                    diff.items.push(DiffItem {
                        item_type: if has_rename { DiffItemType::RenameColumn } else { DiffItemType::AlterTable },
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
            if matched_created_tables.contains(created) { continue; }
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
            let view_re = Regex::new(r"(?i)^(\s*CREATE\s+)VIEW\b").unwrap();
            let safe_sql = view_re.replace(shadow_def, "${1}OR REPLACE VIEW").to_string();
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
            let safe_sql = macro_re.replace(shadow_def, "${1}OR REPLACE MACRO").to_string();
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
                    target_trg.table_name.clone()
                } else {
                    format!("{}.{}", target_trg.schema_name, target_trg.table_name)
                };
                diff.items.push(DiffItem {
                    item_type: DiffItemType::DropTrigger,
                    obj_name: name.clone(),
                    sql: format!("DROP TRIGGER {} ON {};", target_trg.trigger_name, target_table),
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
                target_trg.table_name.clone()
            } else {
                format!("{}.{}", target_trg.schema_name, target_trg.table_name)
            };
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropTrigger,
                obj_name: name.clone(),
                sql: format!("DROP TRIGGER {} ON {};", target_trg.trigger_name, target_table),
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
        
        shadow.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER);".to_string(),
            columns,
            constraints: Vec::new(),
        });
        
        let diff = compute_diff(
            &shadow_schemas, &target_schemas,
            &shadow, &target,
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &std::path::PathBuf::new(),
            &[]
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
            &HashSet::new(), &HashSet::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &shadow_triggers, &target_triggers,
            &std::path::PathBuf::new(),
            &[]
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
            &HashSet::new(), &HashSet::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &target_triggers,
            &std::path::PathBuf::new(),
            &[]
        );
        assert_eq!(diff2.items.len(), 1);
        assert_eq!(diff2.items[0].item_type, DiffItemType::DropTrigger);
        assert_eq!(diff2.items[0].sql, "DROP TRIGGER trg_old ON orders;");
    }

    #[test]
    fn test_validate_target_catalog_and_isolation() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE memory_tbl (id INT);").unwrap();
        
        // Default target catalog
        let default_cat = validate_target_catalog(&conn, None, false).unwrap();
        assert_eq!(default_cat, "memory");

        // Specific target catalog
        let val_cat = validate_target_catalog(&conn, Some("memory"), true).unwrap();
        assert_eq!(val_cat, "memory");

        // Missing catalog error
        let err = validate_target_catalog(&conn, Some("nonexistent"), false);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("not found on connection"));

        // Schema isolation test
        let schema_mem = get_schema(&conn, Some("memory")).unwrap();
        assert!(schema_mem.contains_key("memory_tbl"));

        // Attach secondary database target_db
        conn.execute_batch("ATTACH '' AS target_db; USE target_db;").unwrap();
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
        conn.execute_batch("CREATE TABLE test_users (id INT, name VARCHAR);").unwrap();
        // Test DuckDB native trigger syntax: statement follows FOR EACH ROW / STATEMENT directly
        conn.execute_batch("CREATE TABLE audit_log (id INT);").unwrap();
        let res = conn.execute_batch("CREATE TRIGGER trg1 AFTER INSERT ON test_users FOR EACH ROW INSERT INTO audit_log VALUES (NEW.id);");
        println!("Native DuckDB trigger result: {:?}", res);
        assert!(res.is_ok());

        // Check duckdb_triggers contents
        let mut stmt = conn.prepare("SELECT schema_name, trigger_name, table_name, sql FROM duckdb_triggers()").unwrap();
        let rows: Vec<_> = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))).unwrap().flatten().collect();
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
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
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
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                let cols: Vec<_> = rows.flatten().collect();
                println!("duckdb_schemas() schema: {:?}", cols);
            }
        }
        let schema_pairs: Vec<(String, Option<String>)> = conn.prepare("SELECT schema_name, parent_schema FROM duckdb_schemas() WHERE internal = false")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1).ok()))).unwrap().flatten().collect();
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

        target.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER, name VARCHAR);".to_string(),
            columns: cols.clone(),
            constraints: Vec::new(),
        });

        shadow.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR);".to_string(),
            columns: cols,
            constraints: vec![ConstraintSchema {
                constraint_type: "PRIMARY KEY".to_string(),
                constraint_name: Some("users_id_pkey".to_string()),
                columns: vec!["id".to_string()],
                expression: None,
                referenced_table: None,
                referenced_columns: Vec::new(),
            }],
        });

        let diff = compute_diff(
            &HashSet::new(), &HashSet::new(),
            &shadow, &target,
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &std::path::PathBuf::new(),
            &[]
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

        target.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR);".to_string(),
            columns: cols.clone(),
            constraints: vec![pk.clone()],
        });

        shadow.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR UNIQUE);".to_string(),
            columns: cols,
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
        });

        let diff = compute_diff(
            &HashSet::new(), &HashSet::new(),
            &shadow, &target,
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &std::path::PathBuf::new(),
            &[]
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

        target.insert("orders".to_string(), TableSchema {
            create_sql: "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER);".to_string(),
            columns: cols.clone(),
            constraints: vec![pk.clone()],
        });

        shadow.insert("orders".to_string(), TableSchema {
            create_sql: "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id));".to_string(),
            columns: cols,
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
            &HashSet::new(), &HashSet::new(),
            &shadow, &target,
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &std::path::PathBuf::new(),
            &[]
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::RecreateTable);
        assert!(diff.items[0].sql.contains("BEGIN TRANSACTION;"));
        assert!(diff.items[0].sql.contains("CREATE TABLE orders__duckpack_swap"));
        assert!(diff.items[0].sql.contains("INSERT INTO orders__duckpack_swap"));
        assert!(diff.items[0].sql.contains("DROP TABLE orders;"));
        assert!(diff.items[0].sql.contains("ALTER TABLE orders__duckpack_swap RENAME TO orders;"));
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

        target.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR UNIQUE);".to_string(),
            columns: cols.clone(),
            constraints: vec![pk.clone(), uq],
        });

        // Shadow removed the UNIQUE constraint
        shadow.insert("users".to_string(), TableSchema {
            create_sql: "CREATE TABLE users (id INTEGER PRIMARY KEY, email VARCHAR);".to_string(),
            columns: cols,
            constraints: vec![pk],
        });

        let diff = compute_diff(
            &HashSet::new(), &HashSet::new(),
            &shadow, &target,
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &HashMap::new(), &HashMap::new(),
            &std::path::PathBuf::new(),
            &[]
        );

        assert_eq!(diff.items.len(), 1);
        assert_eq!(diff.items[0].item_type, DiffItemType::RecreateTable);
    }

    #[test]
    fn test_get_schema_duckdb_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE users (id INT PRIMARY KEY, email VARCHAR UNIQUE);").unwrap();
        let schema = get_schema(&conn, Some("memory")).unwrap();
        assert!(schema.contains_key("users"));
        let users_tbl = &schema["users"];
        assert_eq!(users_tbl.columns.len(), 2);
        assert_eq!(users_tbl.constraints.len(), 2);
        assert!(users_tbl.constraints.iter().any(|c| c.constraint_type == "PRIMARY KEY" && c.columns == vec!["id"]));
        assert!(users_tbl.constraints.iter().any(|c| c.constraint_type == "UNIQUE" && c.columns == vec!["email"]));
    }
}

