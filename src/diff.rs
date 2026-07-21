use anyhow::Result;
use duckdb::Connection;
use std::collections::HashMap;
use std::path::Path;
use regex::Regex;
use std::sync::OnceLock;

/// Reads a SQL file and dynamically interpolates any `${VARIABLE_NAME}` templates 
/// with the system's current environment variables (loaded from .env).
/// This is used for multi-tenant or multi-environment deployments.
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

/// Represents the resolved structure of a DuckDB table.
/// Contains the raw creation SQL and a map of column names to their data types.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub create_sql: String,
    pub columns: HashMap<String, String>,
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
    CreateView,
    DropView,
    CreateMacro,
    DropMacro,
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
#[derive(Debug)]
pub struct DiffResult {
    pub items: Vec<DiffItem>,
}

impl DiffResult {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Extracts the entire table schema from a DuckDB connection.
/// It queries `sqlite_master` for table definitions and `duckdb_columns()` for specific datatypes.
pub fn get_schema(conn: &Connection) -> Result<HashMap<String, TableSchema>> {
    let mut stmt = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, COALESCE(sql, '/* Remote table: ' || table_name || ' */') FROM duckdb_tables() WHERE internal = false")?;
    let mut tables: HashMap<String, TableSchema> = HashMap::new();
    
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (table_name, sql) = row?;
        tables.insert(table_name.clone(), TableSchema {
            create_sql: sql,
            columns: HashMap::new(),
        });
    }

    let mut col_stmt = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN table_name ELSE schema_name || '.' || table_name END, column_name, data_type FROM duckdb_columns() WHERE internal = false")?;
    let col_rows = col_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;

    for row in col_rows {
        let (table_name, column_name, data_type) = row?;
        if let Some(table) = tables.get_mut(&table_name) {
            table.columns.insert(column_name, data_type);
        }
    }
    
    Ok(tables)
}

/// Extracts all view definitions from a DuckDB connection using `sqlite_master`.
pub fn get_views(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN view_name ELSE schema_name || '.' || view_name END, COALESCE(sql, '/* Remote view: ' || view_name || ' */') FROM duckdb_views() WHERE internal = false")?;
    
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut views = HashMap::new();
    for row in rows {
        let (name, sql) = row?;
        views.insert(name, sql);
    }
    Ok(views)
}

/// Extracts all macro (function) definitions from a DuckDB connection using `duckdb_functions()`.
pub fn get_macros(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END, macro_definition || CAST(parameters AS VARCHAR) FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut macros = HashMap::new();
    for row in rows {
        let (name, def) = row?;
        macros.insert(name, def);
    }
    Ok(macros)
}

/// Extracts all schema names (excluding system schemas) from a DuckDB connection.
pub fn get_schemas(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('information_schema', 'pg_catalog', 'main')")?;
    let rows = stmt.query_map([], |row| Ok(row.get::<_, String>(0)?))?;
    let mut schemas = std::collections::HashSet::new();
    for row in rows {
        schemas.insert(row?);
    }
    Ok(schemas)
}

/// The core diffing engine. 
/// Compares the `shadow` database (desired state) against the `target` database (current state)
/// and calculates the exact delta (adding tables, renaming columns, altering types, etc.).
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
    project_dir: &std::path::PathBuf,
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

            if !safe_sqls.is_empty() {
                let has_rename = safe_sqls.iter().any(|s| s.contains("RENAME COLUMN"));
                diff.items.push(DiffItem {
                    item_type: if has_rename { DiffItemType::RenameColumn } else { DiffItemType::AlterTable },
                    obj_name: table_name.clone(),
                    sql: safe_sqls.join("\n"),
                    old_def: Some(target_table.create_sql.clone()),
                    new_def: Some(shadow_table.create_sql.clone()),
                    selected: true,
                    locked: false,
                    ignored: is_ignored(table_name),
                });
            }

            if !destructive_sqls.is_empty() {
                diff.items.push(DiffItem {
                    item_type: DiffItemType::DropColumn,
                    obj_name: table_name.clone(),
                    sql: destructive_sqls.join("\n"),
                    old_def: Some(target_table.create_sql.clone()),
                    new_def: Some(shadow_table.create_sql.clone()),
                    selected: true,
                    locked: false,
                    ignored: is_ignored(table_name),
                });
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
                diff.items.push(DiffItem {
                    item_type: DiffItemType::RenameTable,
                    obj_name: format!("{} -> {}", dropped, created),
                    sql: format!("ALTER TABLE {} RENAME TO {};", dropped, created),
                    old_def: Some(target_table.create_sql.clone()),
                    new_def: Some(shadow_table.create_sql.clone()),
                    selected: true,
                    locked: false,
                    ignored: is_ignored(dropped) || is_ignored(created),
                });
                break;
            }
        }
    }

    for created in &created_tables {
        if !matched_created_tables.contains(created) {
            let shadow_table = shadow.get(created).unwrap();
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateTable,
                obj_name: created.clone(),
                sql: shadow_table.create_sql.clone(),
                old_def: Some(String::new()),
                new_def: Some(shadow_table.create_sql.clone()),
                selected: true,
                locked: false,
                ignored: is_ignored(created),
            });
        }
    }

    for dropped in &dropped_tables {
        if !matched_dropped_tables.contains(dropped) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropTable,
                obj_name: dropped.clone(),
                sql: format!("DROP TABLE {}", dropped),
                old_def: Some(target.get(dropped).unwrap().create_sql.clone()),
                new_def: Some(String::new()),
                selected: true,
                locked: false,
                ignored: is_ignored(dropped),
            });
        }
    }
    
    // Add views and macros
    for (name, shadow_def) in shadow_views {
        let target_def = target_views.get(name);
        
        if target_def.is_none() || target_def != Some(shadow_def) {
            let view_re = Regex::new(r"(?i)^(\s*CREATE\s+)VIEW\b").unwrap();
            let safe_sql = view_re.replace(shadow_def, "${1}OR REPLACE VIEW").to_string();
            
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateView,
                obj_name: name.clone(),
                sql: safe_sql,
                old_def: target_def.cloned(),
                new_def: Some(shadow_def.clone()),
                selected: true,
                locked: false,
                ignored: is_ignored(name),
            });
        }
    }
    
    for name in target_views.keys() {
        if !shadow_views.contains_key(name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropView,
                obj_name: name.clone(),
                sql: format!("DROP VIEW {}", name),
                old_def: Some(target_views.get(name).unwrap().clone()),
                new_def: Some(String::new()),
                selected: true,
                locked: false,
                ignored: is_ignored(name),
            });
        }
    }

    for (name, shadow_def) in shadow_macros {
        let target_def = target_macros.get(name);

        if target_def.is_none() || target_def != Some(shadow_def) {
            let macro_re = Regex::new(r"(?i)^(\s*CREATE\s+)MACRO\b").unwrap();
            let safe_sql = macro_re.replace(shadow_def, "${1}OR REPLACE MACRO").to_string();

            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateMacro,
                obj_name: name.clone(),
                sql: safe_sql,
                old_def: target_def.cloned(),
                new_def: Some(shadow_def.clone()),
                selected: true,
                locked: false,
                ignored: is_ignored(name),
            });
        }
    }
    
    for name in target_macros.keys() {
        if !shadow_macros.contains_key(name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropMacro,
                obj_name: name.clone(),
                sql: format!("DROP MACRO {}", name),
                old_def: Some(target_macros.get(name).unwrap().clone()),
                new_def: Some(String::new()),
                selected: true,
                locked: false,
                ignored: is_ignored(name),
            });
        }
    }

    diff
}
