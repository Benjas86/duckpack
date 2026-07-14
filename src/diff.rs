use anyhow::Result;
use duckdb::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub create_sql: String,
    pub columns: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffItemType {
    CreateTable,
    AlterTable,
    DropTable,
    DropColumn,
    CreateView,
    DropView,
    CreateMacro,
    DropMacro,
}

#[derive(Debug, Clone)]
pub struct DiffItem {
    pub item_type: DiffItemType,
    pub obj_name: String,
    pub sql: String,
    pub selected: bool,
    pub locked: bool,
}

#[derive(Debug)]
pub struct DiffResult {
    pub items: Vec<DiffItem>,
}

impl DiffResult {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

pub fn get_schema(conn: &Connection) -> Result<HashMap<String, TableSchema>> {
    let mut stmt = conn.prepare("SELECT table_name, sql FROM duckdb_tables() WHERE schema_name = 'main' AND internal = false")?;
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

    let mut col_stmt = conn.prepare("SELECT table_name, column_name, data_type FROM duckdb_columns() WHERE schema_name = 'main' AND internal = false")?;
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

pub fn get_views(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT view_name, sql FROM duckdb_views() WHERE internal = false")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut views = HashMap::new();
    for row in rows {
        let (name, sql) = row?;
        views.insert(name, sql);
    }
    Ok(views)
}

pub fn get_macros(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT function_name, macro_definition || CAST(parameters AS VARCHAR) FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut macros = HashMap::new();
    for row in rows {
        let (name, def) = row?;
        macros.insert(name, def);
    }
    Ok(macros)
}

pub fn compute_diff(
    shadow: &HashMap<String, TableSchema>, 
    target: &HashMap<String, TableSchema>, 
    shadow_views: &HashMap<String, String>,
    target_views: &HashMap<String, String>,
    shadow_macros: &HashMap<String, String>,
    target_macros: &HashMap<String, String>,
    project_dir: &std::path::PathBuf
) -> DiffResult {
    let mut diff = DiffResult {
        items: Vec::new(),
    };

    for (table_name, shadow_table) in shadow.iter() {
        if let Some(target_table) = target.get(table_name) {
            for (col_name, shadow_type) in shadow_table.columns.iter() {
                if let Some(target_type) = target_table.columns.get(col_name) {
                    if shadow_type != target_type {
                        diff.items.push(DiffItem {
                            item_type: DiffItemType::AlterTable,
                            obj_name: table_name.clone(),
                            sql: format!("ALTER TABLE {} ALTER {} TYPE {}", table_name, col_name, shadow_type),
                            selected: true,
                            locked: false,
                        });
                    }
                } else {
                    diff.items.push(DiffItem {
                        item_type: DiffItemType::AlterTable,
                        obj_name: table_name.clone(),
                        sql: format!("ALTER TABLE {} ADD COLUMN {} {}", table_name, col_name, shadow_type),
                        selected: true,
                        locked: false,
                    });
                }
            }
            
            for col_name in target_table.columns.keys() {
                if !shadow_table.columns.contains_key(col_name) {
                    diff.items.push(DiffItem {
                        item_type: DiffItemType::DropColumn,
                        obj_name: table_name.clone(),
                        sql: format!("ALTER TABLE {} DROP COLUMN {}", table_name, col_name),
                        selected: true,
                        locked: false,
                    });
                }
            }
        } else {
            diff.items.push(DiffItem {
                item_type: DiffItemType::CreateTable,
                obj_name: table_name.clone(),
                sql: shadow_table.create_sql.clone(),
                selected: true,
                locked: false,
            });
        }
    }

    for table_name in target.keys() {
        if !shadow.contains_key(table_name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropTable,
                obj_name: table_name.clone(),
                sql: format!("DROP TABLE {}", table_name),
                selected: true,
                locked: false,
            });
        }
    }
    
    // Add views and macros
    let views_dir = project_dir.join("views");
    if views_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(views_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().unwrap_or_default() == "sql" {
                    if let Ok(sql) = std::fs::read_to_string(&path) {
                        let name = path.file_stem().unwrap().to_string_lossy().to_string();
                        let shadow_def = shadow_views.get(&name);
                        let target_def = target_views.get(&name);
                        
                        if target_def.is_none() || target_def != shadow_def {
                            diff.items.push(DiffItem {
                                item_type: DiffItemType::CreateView,
                                obj_name: name,
                                sql,
                                selected: true,
                                locked: false,
                            });
                        }
                    }
                }
            }
        }
    }
    
    for name in target_views.keys() {
        if !shadow_views.contains_key(name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropView,
                obj_name: name.clone(),
                sql: format!("DROP VIEW {}", name),
                selected: true,
                locked: false,
            });
        }
    }

    let macros_dir = project_dir.join("macros");
    if macros_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(macros_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().unwrap_or_default() == "sql" {
                    if let Ok(sql) = std::fs::read_to_string(&path) {
                        let name = path.file_stem().unwrap().to_string_lossy().to_string();
                        let shadow_def = shadow_macros.get(&name);
                        let target_def = target_macros.get(&name);

                        if target_def.is_none() || target_def != shadow_def {
                            diff.items.push(DiffItem {
                                item_type: DiffItemType::CreateMacro,
                                obj_name: name,
                                sql,
                                selected: true,
                                locked: false,
                            });
                        }
                    }
                }
            }
        }
    }
    
    for name in target_macros.keys() {
        if !shadow_macros.contains_key(name) {
            diff.items.push(DiffItem {
                item_type: DiffItemType::DropMacro,
                obj_name: name.clone(),
                sql: format!("DROP MACRO {}", name),
                selected: true,
                locked: false,
            });
        }
    }

    diff
}
