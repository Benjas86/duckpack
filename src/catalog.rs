use duckdb::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Represents an individual table across any attached DuckDB database/catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct TableEntry {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub row_count: Option<i64>,
    pub is_temporary: bool,
}

impl TableEntry {
    pub fn display_name(&self) -> String {
        if self.schema == "main" {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    pub fn qualified_name(&self) -> String {
        format!("{}.{}.{}", self.database, self.schema, self.name)
    }
}

/// Represents an individual view across any attached DuckDB database/catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewEntry {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub is_temporary: bool,
}

impl ViewEntry {
    pub fn display_name(&self) -> String {
        if self.schema == "main" {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    pub fn qualified_name(&self) -> String {
        format!("{}.{}.{}", self.database, self.schema, self.name)
    }
}

/// Represents an individual trigger across any attached DuckDB database/catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerEntry {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub table_name: String,
}

impl TriggerEntry {
    pub fn display_name(&self) -> String {
        if self.schema == "main" {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    pub fn qualified_name(&self) -> String {
        format!("{}.{}.{}", self.database, self.schema, self.name)
    }
}

/// Metadata holding info on an attached database catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct DatabaseInfo {
    pub name: String,
    pub db_type: String,
    pub path: Option<String>,
    pub is_active: bool,
    pub readonly: bool,
    pub tables: Vec<TableEntry>,
    pub views: Vec<ViewEntry>,
    pub triggers: Vec<TriggerEntry>,
}

/// Metadata holding all discoverable entities in the database and project.
#[derive(Clone, Debug)]
pub struct ExplorerMetadata {
    pub databases: Vec<DatabaseInfo>,
    pub active_database: String,
    pub tables: Vec<String>,
    pub table_counts: HashMap<String, i64>,
    pub views: Vec<String>,
    pub temp_tables: HashSet<String>,
    pub temp_views: HashSet<String>,
    pub macros: Vec<String>,
    pub triggers: Vec<String>,
    pub queries: Vec<String>,
    pub dictionary: Vec<String>,
}

/// Formats an integer count with comma separators and singular/plural suffixes (e.g. "1 row", "10,000 rows").
pub fn format_row_count(count: i64) -> String {
    if count == 1 {
        "1 row".to_string()
    } else {
        let s = count.to_string();
        let mut result = String::new();
        let len = s.len();
        for (idx, ch) in s.chars().enumerate() {
            if idx > 0 && (len - idx) % 3 == 0 {
                result.push(',');
            }
            result.push(ch);
        }
        format!("{} rows", result)
    }
}

/// Loads column names and types for a specific database, schema, and table/view.
pub fn load_table_columns_for(
    db: &str,
    schema: &str,
    table: &str,
    conn: &Connection,
    is_quack: bool,
) -> Vec<(String, String)> {
    let mut cols = Vec::new();

    // 1. Direct query on db if it is target_db
    if is_quack && db == "target_db" {
        let col_query = format!(
            "SELECT column_name, data_type FROM target_db.query('SELECT column_name, data_type FROM duckdb_columns() WHERE schema_name = ''{}'' AND table_name = ''{}'' ORDER BY column_index')",
            schema.replace('\'', "''"),
            table.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        cols.push((name, typ));
                    }
                }
            }
        }
    }

    // 2. Check duckdb_columns() via target_db.query (for remote multi-database mode)
    if cols.is_empty() && is_quack {
        let col_query = format!(
            "SELECT column_name, data_type FROM target_db.query('SELECT column_name, data_type FROM duckdb_columns() WHERE (database_name = ''{}'' OR database_name = ''temp'') AND schema_name = ''{}'' AND table_name = ''{}'' ORDER BY column_index')",
            db.replace('\'', "''"),
            schema.replace('\'', "''"),
            table.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        cols.push((name, typ));
                    }
                }
            }
        }
    }

    // 3. Nested quack query if on remote multi-db (e.g. remote_db attached inside target_db)
    if cols.is_empty() && is_quack && db != "target_db" && db != "memory" {
        let nested_col_query = format!(
            "SELECT * FROM target_db.query('SELECT column_name, data_type FROM {}.query(''SELECT column_name, data_type FROM duckdb_columns() WHERE table_name = ''''{}'''' ORDER BY column_index'')')",
            db.replace('\'', "''"),
            table.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&nested_col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        cols.push((name, typ));
                    }
                }
            }
        }
    }

    // 4. Query {db}.query directly if db is an attached quack DB on conn
    if cols.is_empty() {
        let direct_quack_col = format!(
            "SELECT column_name, data_type FROM {}.query('SELECT column_name, data_type FROM duckdb_columns() WHERE table_name = ''{}'' ORDER BY column_index')",
            db.replace('\'', "''"),
            table.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&direct_quack_col) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        cols.push((name, typ));
                    }
                }
            }
        }
    }

    // 5. Query local duckdb_columns() on conn
    if cols.is_empty() {
        let col_query = format!(
            "SELECT column_name, data_type FROM duckdb_columns() WHERE (database_name = '{}' OR database_name = 'temp') AND schema_name = '{}' AND table_name = '{}' ORDER BY column_index",
            db.replace('\'', "''"),
            schema.replace('\'', "''"),
            table.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        cols.push((name, typ));
                    }
                }
            }
        }
    }

    // 6. PRAGMA table_info fallback
    if cols.is_empty() {
        let qualified = if schema == "main" {
            format!("{}.{}", db, table)
        } else {
            format!("{}.{}.{}", db, schema, table)
        };
        let safe_qualified = qualified.replace('\'', "''");
        let pragma_queries = if is_quack && db == "target_db" {
            vec![
                format!("SELECT * FROM target_db.query('PRAGMA table_info(''{table}'')')"),
                format!("PRAGMA table_info('{}')", safe_qualified),
            ]
        } else if is_quack {
            vec![
                format!("SELECT * FROM target_db.query('PRAGMA table_info(''{safe_qualified}'')')"),
                format!("PRAGMA table_info('{}')", safe_qualified),
            ]
        } else {
            vec![
                format!("PRAGMA table_info('{}')", safe_qualified),
            ]
        };

        for query in &pragma_queries {
            if let Ok(mut stmt) = conn.prepare(query) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let Ok(col_name) = row.get::<_, String>(1) {
                            let data_type: String = row.get(2).unwrap_or_else(|_| "UNKNOWN".to_string());
                            cols.push((col_name, data_type));
                        }
                    }
                }
            }
            if !cols.is_empty() {
                break;
            }
        }
    }

    if cols.is_empty() {
        // Fallback to simple table name
        let simple_name = table.replace('\'', "''");
        if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info('{}')", simple_name)) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(col_name) = row.get::<_, String>(1) {
                        let data_type: String = row.get(2).unwrap_or_else(|_| "UNKNOWN".to_string());
                        cols.push((col_name, data_type));
                    }
                }
            }
        }
    }

    cols
}

/// Discovers all databases, tables, views, macros, and queries in the DuckDB environment.
pub fn load_explorer_metadata(
    conn: &Connection,
    project_dir: &Path,
    is_quack: bool,
) -> ExplorerMetadata {
    let mut remote_dbs: Vec<(String, String, Option<String>, bool)> = Vec::new();
    if is_quack {
        let quack_db_sql = "SELECT database_name, type, path, readonly FROM target_db.query('SELECT database_name, type, path, readonly FROM duckdb_databases() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_db_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        let path: Option<String> = row.get(2).ok();
                        let readonly: bool = row.get(3).unwrap_or(false);
                        remote_dbs.push((name, typ, path, readonly));
                    }
                }
            }
        }
    }

    let is_remote_multi_db = remote_dbs.len() > 1;

    let active_database: String = if is_remote_multi_db {
        conn.query_row(
            "SELECT * FROM target_db.query('SELECT current_database()')",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "memory".to_string())
    } else if is_quack {
        "target_db".to_string()
    } else {
        conn.query_row("SELECT current_database();", [], |r| r.get(0))
            .unwrap_or_else(|_| "memory".to_string())
    };

    let mut raw_dbs: Vec<(String, String, Option<String>, bool)> = Vec::new();
    if is_remote_multi_db {
        raw_dbs = remote_dbs;
    } else {
        if let Ok(mut stmt) = conn.prepare("SELECT database_name, type, path, readonly FROM duckdb_databases() WHERE internal = false") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        let path: Option<String> = row.get(2).ok();
                        let readonly: bool = row.get(3).unwrap_or(false);
                        raw_dbs.push((name, typ, path, readonly));
                    }
                }
            }
        }
    }
    if raw_dbs.is_empty() {
        raw_dbs.push((active_database.clone(), "duckdb".to_string(), None, false));
    }

    let mut all_tables: Vec<TableEntry> = Vec::new();
    if is_quack {
        let quack_tbl_sql = "SELECT database_name, schema_name, table_name, estimated_size, temporary FROM target_db.query('SELECT database_name, schema_name, table_name, estimated_size, temporary FROM duckdb_tables() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_tbl_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                        if table.starts_with("__duckpack_") {
                            continue;
                        }
                        let est: Option<i64> = row.get(3).ok();
                        let is_temp: bool = row.get(4).unwrap_or(false) || db == "temp";
                        all_tables.push(TableEntry {
                            database: if is_temp { active_database.clone() } else if !is_remote_multi_db { "target_db".to_string() } else { db },
                            schema,
                            name: table,
                            row_count: est,
                            is_temporary: is_temp,
                        });
                    }
                }
            }
        }
    } else {
        // Query tables on conn (local and non-quack attached databases)
        if let Ok(mut stmt) = conn.prepare("SELECT database_name, schema_name, table_name, estimated_size, temporary FROM duckdb_tables() WHERE internal = false") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                        if table.starts_with("__duckpack_") {
                            continue;
                        }
                        let est: Option<i64> = row.get(3).ok();
                        let is_temp: bool = row.get(4).unwrap_or(false) || db == "temp";
                        all_tables.push(TableEntry {
                            database: if is_temp { active_database.clone() } else { db },
                            schema,
                            name: table,
                            row_count: est,
                            is_temporary: is_temp,
                        });
                    }
                }
            }
        }
    }

    // Query tables from attached quack databases
    for (name, typ, _, _) in &raw_dbs {
        if typ.eq_ignore_ascii_case("quack") {
            let safe_name = name.replace('\'', "''");
            let sql = if is_remote_multi_db {
                format!(
                    "SELECT * FROM target_db.query('SELECT ''{}'' AS database_name, schema_name, table_name, estimated_size, temporary FROM {}.query(''SELECT schema_name, table_name, estimated_size, temporary FROM duckdb_tables() WHERE internal = false'')')",
                    safe_name, safe_name
                )
            } else {
                format!(
                    "SELECT '{}' AS database_name, schema_name, table_name, estimated_size, temporary FROM {}.query('SELECT schema_name, table_name, estimated_size, temporary FROM duckdb_tables() WHERE internal = false')",
                    safe_name, safe_name
                )
            };
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(db), Ok(schema), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                            if table.starts_with("__duckpack_") {
                                continue;
                            }
                            let est: Option<i64> = row.get(3).ok();
                            let is_temp: bool = row.get(4).unwrap_or(false) || db == "temp";
                            all_tables.push(TableEntry {
                                database: if is_temp { active_database.clone() } else { db },
                                schema,
                                name: table,
                                row_count: est,
                                is_temporary: is_temp,
                            });
                        }
                    }
                }
            }
        }
    }

    // Ensure all local temporary tables directly on conn are merged into all_tables under active_database
    if let Ok(mut stmt) = conn.prepare("SELECT schema_name, table_name, estimated_size FROM duckdb_tables() WHERE (temporary = true OR database_name = 'temp') AND internal = false") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(schema), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    if table.starts_with("__duckpack_") {
                        continue;
                    }
                    let est: Option<i64> = row.get(2).ok();
                    if !all_tables.iter().any(|t| t.is_temporary && t.name.eq_ignore_ascii_case(&table) && t.schema.eq_ignore_ascii_case(&schema)) {
                        all_tables.push(TableEntry {
                            database: active_database.clone(),
                            schema,
                            name: table,
                            row_count: est,
                            is_temporary: true,
                        });
                    }
                }
            }
        }
    }

    // Information schema fallback if tables still empty
    if all_tables.is_empty() {
        if let Ok(mut stmt) = conn.prepare("SELECT table_schema, table_name FROM information_schema.tables WHERE table_type = 'BASE TABLE' AND table_schema NOT IN ('information_schema', 'pg_catalog')") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        all_tables.push(TableEntry {
                            database: active_database.clone(),
                            schema,
                            name,
                            row_count: None,
                            is_temporary: false,
                        });
                    }
                }
            }
        }
    }

    let mut all_views: Vec<ViewEntry> = Vec::new();
    if is_quack {
        let quack_view_sql = "SELECT database_name, schema_name, view_name, temporary FROM target_db.query('SELECT database_name, schema_name, view_name, temporary FROM duckdb_views() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_view_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                        if name.starts_with("__duckpack_") {
                            continue;
                        }
                        let is_temp: bool = row.get(3).unwrap_or(false) || db == "temp";
                        all_views.push(ViewEntry {
                            database: if is_temp { active_database.clone() } else if !is_remote_multi_db { "target_db".to_string() } else { db },
                            schema,
                            name,
                            is_temporary: is_temp,
                        });
                    }
                }
            }
        }
    } else {
        if let Ok(mut stmt) = conn.prepare("SELECT database_name, schema_name, view_name, temporary FROM duckdb_views() WHERE internal = false") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                        if name.starts_with("__duckpack_") {
                            continue;
                        }
                        let is_temp: bool = row.get(3).unwrap_or(false) || db == "temp";
                        all_views.push(ViewEntry {
                            database: if is_temp { active_database.clone() } else { db },
                            schema,
                            name,
                            is_temporary: is_temp,
                        });
                    }
                }
            }
        }
    }

    // Query views from attached quack databases
    for (name, typ, _, _) in &raw_dbs {
        if typ.eq_ignore_ascii_case("quack") {
            let safe_name = name.replace('\'', "''");
            let sql = if is_remote_multi_db {
                format!(
                    "SELECT * FROM target_db.query('SELECT ''{}'' AS database_name, schema_name, view_name, temporary FROM {}.query(''SELECT schema_name, view_name, temporary FROM duckdb_views() WHERE internal = false'')')",
                    safe_name, safe_name
                )
            } else {
                format!(
                    "SELECT '{}' AS database_name, schema_name, view_name, temporary FROM {}.query('SELECT schema_name, view_name, temporary FROM duckdb_views() WHERE internal = false')",
                    safe_name, safe_name
                )
            };
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(db), Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                            if name.starts_with("__duckpack_") {
                                continue;
                            }
                            let is_temp: bool = row.get(3).unwrap_or(false) || db == "temp";
                            all_views.push(ViewEntry {
                                database: if is_temp { active_database.clone() } else { db },
                                schema,
                                name,
                                is_temporary: is_temp,
                            });
                        }
                    }
                }
            }
        }
    }

    // Ensure all local temporary views directly on conn are merged into all_views under active_database
    if let Ok(mut stmt) = conn.prepare("SELECT schema_name, view_name FROM duckdb_views() WHERE (temporary = true OR database_name = 'temp') AND internal = false") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    if name.starts_with("__duckpack_") {
                        continue;
                    }
                    if !all_views.iter().any(|v| v.is_temporary && v.name.eq_ignore_ascii_case(&name) && v.schema.eq_ignore_ascii_case(&schema)) {
                        all_views.push(ViewEntry {
                            database: active_database.clone(),
                            schema,
                            name,
                            is_temporary: true,
                        });
                    }
                }
            }
        }
    }

    // Information schema fallback if views still empty
    if all_views.is_empty() {
        if let Ok(mut stmt) = conn.prepare("SELECT table_schema, table_name FROM information_schema.tables WHERE table_type = 'VIEW' AND table_schema NOT IN ('information_schema', 'pg_catalog')") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(schema), Ok(name)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        if name.starts_with("__duckpack_") {
                            continue;
                        }
                        all_views.push(ViewEntry {
                            database: active_database.clone(),
                            schema,
                            name,
                            is_temporary: false,
                        });
                    }
                }
            }
        }
    }

    let mut all_triggers: Vec<TriggerEntry> = Vec::new();
    if is_quack {
        let quack_trg_sql = "SELECT database_name, schema_name, trigger_name, table_name FROM target_db.query('SELECT database_name, schema_name, trigger_name, table_name FROM duckdb_triggers()')";
        if let Ok(mut stmt) = conn.prepare(quack_trg_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(trg), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3)) {
                        all_triggers.push(TriggerEntry {
                            database: if !is_remote_multi_db { "target_db".to_string() } else { db },
                            schema,
                            name: trg,
                            table_name: table,
                        });
                    }
                }
            }
        }
    } else if let Ok(mut stmt) = conn.prepare("SELECT database_name, schema_name, trigger_name, table_name FROM duckdb_triggers()") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(db), Ok(schema), Ok(trg), Ok(table)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3)) {
                    all_triggers.push(TriggerEntry {
                        database: db,
                        schema,
                        name: trg,
                        table_name: table,
                    });
                }
            }
        }
    }

    let mut databases: Vec<DatabaseInfo> = Vec::new();
    let is_remote_quack = is_quack || raw_dbs.iter().any(|d| d.0 == "target_db" || d.1.eq_ignore_ascii_case("quack"));
    for (name, db_type, path, readonly) in &raw_dbs {
        let mut db_tables: Vec<TableEntry> = all_tables.iter().filter(|t| &t.database == name).cloned().collect();
        let mut db_views: Vec<ViewEntry> = all_views.iter().filter(|v| &v.database == name).cloned().collect();
        let mut db_triggers: Vec<TriggerEntry> = all_triggers.iter().filter(|t| &t.database == name).cloned().collect();
        db_tables.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        db_views.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        db_triggers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // If 'memory' is empty and not the active database, skip it only in classic single local DB mode
        if !is_remote_quack && raw_dbs.len() <= 1 && name == "memory" && active_database != "memory" && db_tables.is_empty() && db_views.is_empty() && db_triggers.is_empty() {
            continue;
        }

        let is_active = name == &active_database;
        databases.push(DatabaseInfo {
            name: name.clone(),
            db_type: db_type.clone(),
            path: path.clone(),
            is_active,
            readonly: *readonly,
            tables: db_tables,
            views: db_views,
            triggers: db_triggers,
        });
    }

    // Sort: active database first, then alphabetical
    databases.sort_by(|a, b| {
        if a.is_active && !b.is_active {
            std::cmp::Ordering::Less
        } else if !a.is_active && b.is_active {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    // Populate flat lists for single-db mode (or active db)
    let mut tables: Vec<String> = Vec::new();
    let mut table_counts: HashMap<String, i64> = HashMap::new();
    let mut views: Vec<String> = Vec::new();
    let mut triggers: Vec<String> = Vec::new();
    let mut temp_tables: HashSet<String> = HashSet::new();
    let mut temp_views: HashSet<String> = HashSet::new();

    for t in &all_tables {
        if t.is_temporary {
            temp_tables.insert(t.name.clone());
            temp_tables.insert(t.display_name());
        }
    }
    for v in &all_views {
        if v.is_temporary {
            temp_views.insert(v.name.clone());
            temp_views.insert(v.display_name());
        }
    }

    if let Some(active_info) = databases.iter().find(|d| d.is_active).or_else(|| databases.first()) {
        for t in &active_info.tables {
            let disp = t.display_name();
            tables.push(disp.clone());
            if let Some(c) = t.row_count {
                table_counts.insert(disp, c);
            }
        }
        for v in &active_info.views {
            views.push(v.display_name());
        }
        for trg in &active_info.triggers {
            triggers.push(format!("{} (on {})", trg.display_name(), trg.table_name));
        }
    }

    let mut macros: Vec<String> = Vec::new();
    if is_remote_multi_db || is_quack {
        let quack_macro_sql = "SELECT function_name FROM target_db.query('SELECT CASE WHEN schema_name = ''main'' THEN function_name ELSE schema_name || ''.'' || function_name END AS function_name FROM duckdb_functions() WHERE function_type = ''macro'' AND internal = false ORDER BY 1')";
        if let Ok(mut stmt) = conn.prepare(quack_macro_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(name) = row.get::<_, String>(0) {
                        if name.starts_with("__duckpack_") {
                            continue;
                        }
                        if !macros.contains(&name) { macros.push(name); }
                    }
                }
            }
        }
    }
    if macros.is_empty() {
        if let Ok(mut stmt) = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false ORDER BY 1") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(name) = row.get::<_, String>(0) {
                        if name.starts_with("__duckpack_") {
                            continue;
                        }
                        macros.push(name);
                    }
                }
            }
        }
    }

    let mut dictionary: Vec<String> = Vec::new();
    for db in &databases {
        dictionary.push(db.name.clone());
        for t in &db.tables {
            dictionary.push(t.name.clone());
            dictionary.push(t.display_name());
            dictionary.push(format!("{}.{}", db.name, t.name));
            dictionary.push(t.qualified_name());
        }
        for v in &db.views {
            dictionary.push(v.name.clone());
            dictionary.push(v.display_name());
            dictionary.push(format!("{}.{}", db.name, v.name));
            dictionary.push(v.qualified_name());
        }
        for trg in &db.triggers {
            dictionary.push(trg.name.clone());
            dictionary.push(trg.display_name());
            dictionary.push(format!("{}.{}", db.name, trg.name));
            dictionary.push(trg.qualified_name());
        }
    }

    // Column names for autocomplete
    if let Ok(mut stmt) = conn.prepare("SELECT database_name, schema_name, table_name, column_name FROM duckdb_columns() WHERE internal = false") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(db), Ok(schema), Ok(table), Ok(col)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3)) {
                    dictionary.push(col.clone());
                    dictionary.push(format!("{}.{}", table, col));
                    dictionary.push(format!("{}.{}.{}", db, table, col));
                    dictionary.push(format!("{}.{}.{}.{}", db, schema, table, col));
                }
            }
        }
    }

    if is_remote_multi_db {
        let quack_col_sql = "SELECT database_name, schema_name, table_name, column_name FROM target_db.query('SELECT database_name, schema_name, table_name, column_name FROM duckdb_columns() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_col_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db), Ok(schema), Ok(table), Ok(col)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3)) {
                        dictionary.push(col.clone());
                        dictionary.push(format!("{}.{}", table, col));
                        dictionary.push(format!("{}.{}.{}", db, table, col));
                        dictionary.push(format!("{}.{}.{}.{}", db, schema, table, col));
                    }
                }
            }
        }
    }

    for (name, typ, _, _) in &raw_dbs {
        if typ.eq_ignore_ascii_case("quack") {
            let safe_name = name.replace('\'', "''");
            let sql = if is_remote_multi_db {
                format!(
                    "SELECT * FROM target_db.query('SELECT ''{}'' AS database_name, schema_name, table_name, column_name FROM {}.query(''SELECT schema_name, table_name, column_name FROM duckdb_columns() WHERE internal = false'')')",
                    safe_name, safe_name
                )
            } else {
                format!(
                    "SELECT '{}' AS database_name, schema_name, table_name, column_name FROM {}.query('SELECT schema_name, table_name, column_name FROM duckdb_columns() WHERE internal = false')",
                    safe_name, safe_name
                )
            };
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(db), Ok(schema), Ok(table), Ok(col)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3)) {
                            dictionary.push(col.clone());
                            dictionary.push(format!("{}.{}", table, col));
                            dictionary.push(format!("{}.{}.{}", db, table, col));
                            dictionary.push(format!("{}.{}.{}.{}", db, schema, table, col));
                        }
                    }
                }
            }
        }
    }

    dictionary.extend(macros.clone());
    dictionary.sort();
    dictionary.dedup();

    let mut queries: Vec<String> = Vec::new();
    let queries_dir = project_dir.join("queries");
    if queries_dir.exists() {
        crate::query::collect_queries_recursive(&queries_dir, &queries_dir, &mut queries);
    }
    queries.sort();

    ExplorerMetadata {
        databases,
        active_database,
        tables,
        table_counts,
        views,
        temp_tables,
        temp_views,
        macros,
        triggers,
        queries,
        dictionary,
    }
}

/// Refreshes the attached remote database catalog so that the client's internal planner and
/// explorer immediately see any new tables, views, or schema changes created on the server.
pub fn refresh_attached_catalog(conn: &Connection, db_path: &str, quack_disable_ssl: bool) {
    if db_path.starts_with("quack:") {
        let disable_ssl = crate::engine::should_disable_quack_ssl(db_path, quack_disable_ssl);
        // Safety probe: Before detaching target_db, test ATTACH under a temporary probe alias.
        // If probing fails (e.g. network timeout, TLS mismatch, server busy), we NEVER detach or drop target_db!
        let probe_sql = format!(
            "ATTACH '{}' AS __duckpack_quack_probe{};",
            db_path.replace('\'', "''"),
            if disable_ssl { " (DISABLE_SSL true)" } else { "" }
        );
        if conn.execute(&probe_sql, []).is_ok() {
            let _ = conn.execute("DETACH __duckpack_quack_probe;", []);
            let attach_sql = crate::engine::build_quack_attach_sql(db_path, disable_ssl);
            let _ = conn.execute("USE memory;", []);
            let _ = conn.execute("DETACH target_db;", []);
            let _ = conn.execute(&attach_sql, []);
            let _ = conn.execute("CONNECT target_db;", []).or_else(|_| conn.execute("USE target_db;", []));
        }
    } else if db_path.starts_with("md:") || db_path.starts_with("motherduck:") {
        let _ = conn.execute("REFRESH DATABASES;", []);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_row_count() {
        assert_eq!(format_row_count(0), "0 rows");
        assert_eq!(format_row_count(1), "1 row");
        assert_eq!(format_row_count(42), "42 rows");
        assert_eq!(format_row_count(1000), "1,000 rows");
        assert_eq!(format_row_count(10000), "10,000 rows");
        assert_eq!(format_row_count(1234567), "1,234,567 rows");
    }

    #[test]
    fn test_duckdb_detach() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        // Switch to memory first, then detach
        conn.execute("USE memory;", []).unwrap();
        conn.execute("DETACH target_db;", []).unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
    }

    #[test]
    fn test_load_explorer_metadata_single_and_attached() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INTEGER, name VARCHAR);", []).unwrap();
        conn.execute("CREATE VIEW active_users AS SELECT * FROM users;", []).unwrap();

        let temp_dir = std::env::temp_dir().join(format!("dp_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Single DB mode
        let meta_single = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta_single.databases.len(), 1);
        assert_eq!(meta_single.tables, vec!["users".to_string()]);
        assert_eq!(meta_single.views, vec!["active_users".to_string()]);

        // 2. Attach a second database
        let attached_db_path = temp_dir.join("analytics.duckdb");
        conn.execute(&format!("ATTACH '{}' AS analytics;", attached_db_path.display()), []).unwrap();
        conn.execute("CREATE TABLE analytics.events (event_id BIGINT, event_name VARCHAR);", []).unwrap();

        let meta_multi = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta_multi.databases.len(), 2);
        assert!(meta_multi.databases.iter().any(|d| d.name == "memory" && d.is_active));
        assert!(meta_multi.databases.iter().any(|d| d.name == "analytics" && !d.is_active));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_quack_attach_preserves_target_db_and_memory() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("CREATE TABLE target_db.remote_tbl (col1 INTEGER, col2 VARCHAR);", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();

        let temp_dir = std::env::temp_dir().join(format!("dp_test_quack_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // When target_db is attached (e.g. Quack attach), both target_db and empty memory are preserved
        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta.databases.len(), 2);
        assert!(meta.databases.iter().any(|d| d.name == "target_db" && d.is_active));
        assert!(meta.databases.iter().any(|d| d.name == "memory" && !d.is_active));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_temporary_tables_and_views_in_explorer() {
        let temp_dir = std::env::temp_dir().join(format!("dp_temp_tbl_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TEMP TABLE staging_tbl (id INT, label VARCHAR);", []).unwrap();
        conn.execute("INSERT INTO staging_tbl VALUES (1, 'alpha'), (2, 'beta');", []).unwrap();
        conn.execute("CREATE TEMP VIEW staging_view AS SELECT id, label FROM staging_tbl;", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert!(meta.temp_tables.contains("staging_tbl"));
        assert!(meta.temp_views.contains("staging_view"));
        assert!(meta.tables.contains(&"staging_tbl".to_string()));
        assert!(meta.views.contains(&"staging_view".to_string()));

        let cols = load_table_columns_for(&meta.active_database, "main", "staging_tbl", &conn, false);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].0, "id");
        assert_eq!(cols[1].0, "label");

        let view_cols = load_table_columns_for(&meta.active_database, "main", "staging_view", &conn, false);
        assert_eq!(view_cols.len(), 2);
        assert_eq!(view_cols[0].0, "id");
        assert_eq!(view_cols[1].0, "label");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_temp_table_creation_on_attached_target_db_preserves_target_db() {
        let temp_dir = std::env::temp_dir().join(format!("dp_temp_target_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        conn.execute("CREATE TABLE target_db.permanent_tbl (id INT, val VARCHAR);", []).unwrap();
        conn.execute("INSERT INTO target_db.permanent_tbl VALUES (1, 'val1'), (2, 'val2');", []).unwrap();

        // Simulate user running: CREATE TEMP TABLE t_tbl AS FROM permanent_tbl LIMIT 10;
        conn.execute("CREATE TEMP TABLE t_tbl AS FROM target_db.permanent_tbl LIMIT 10;", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);

        // target_db must remain the active database!
        assert_eq!(meta.active_database, "target_db");
        assert!(meta.databases.iter().any(|d| d.name == "target_db" && d.is_active));
        assert!(meta.temp_tables.contains("t_tbl"));

        // Both permanent_tbl and t_tbl should be listed
        let target_info = meta.databases.iter().find(|d| d.name == "target_db").unwrap();
        assert!(target_info.tables.iter().any(|t| t.name == "permanent_tbl" && !t.is_temporary));
        assert!(target_info.tables.iter().any(|t| t.name == "t_tbl" && t.is_temporary));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_refresh_attached_catalog_probing_safety_never_drops_target_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        conn.execute("CREATE TABLE target_db.orders (order_id INT);", []).unwrap();

        // Call refresh_attached_catalog with an unreachable Quack server address.
        // Probing should fail and target_db must NOT be detached or dropped!
        refresh_attached_catalog(&conn, "quack:nonexistent.internal.host.xyz:9494", false);

        // Verify target_db is still attached and usable
        let res: i64 = conn.query_row("SELECT count(*) FROM target_db.orders;", [], |r| r.get(0)).unwrap();
        assert_eq!(res, 0);

        let current_db: String = conn.query_row("SELECT current_database();", [], |r| r.get(0)).unwrap();
        assert_eq!(current_db, "target_db");
    }

    #[test]
    fn test_internal_duckpack_bridge_tables_are_excluded_from_catalog() {
        let temp_dir = std::env::temp_dir().join(format!("dp_internal_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INT);", []).unwrap();
        conn.execute("CREATE TABLE __duckpack_temp_bridge_foo (id INT);", []).unwrap();
        conn.execute("CREATE VIEW __duckpack_temp_views_registry AS SELECT 1;", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert!(meta.tables.contains(&"users".to_string()));
        assert!(!meta.tables.iter().any(|t| t.starts_with("__duckpack_")));
        assert!(!meta.views.iter().any(|v| v.starts_with("__duckpack_")));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
