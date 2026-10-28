use anyhow::Result;
use base64::Engine;
use duckdb::Connection;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseEventKind, MouseButton},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, List, ListItem, ListState, Padding, Paragraph, Row, Table, Tabs, Wrap},
    Terminal,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tui_textarea::{CursorMove, TextArea};

pub use crate::catalog::*;
pub use crate::query::*;

/// Manages the currently focused UI pane inside the IDE.
/// Determines which component receives keyboard inputs.
#[derive(PartialEq)]
enum FocusPane {
    Explorer,
    Editor,
    Results,
}

/// Mode for the Explorer sidebar: live database catalog or declarative project files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplorerMode {
    Database,
    Project,
}

/// Categorizes declarative project assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectCategory {
    Schema,
    Table,
    View,
    Macro,
    Trigger,
    Query,
    Seed,
    Config,
}

/// Categorizes the type of entity currently being rendered in the Explorer tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NodeType {
    Database { db_type: String, is_active: bool, path: Option<String> },
    Group,
    Table { database: Option<String>, schema: Option<String>, is_temporary: bool },
    View { database: Option<String>, schema: Option<String>, is_temporary: bool },
    Trigger { database: Option<String>, schema: Option<String>, table: Option<String> },
    Query,
    Macro,
    Column { data_type: String, is_pk: bool, is_fk: bool, fk_target: Option<String> },
    ProjectFile { path: PathBuf, category: ProjectCategory },
    CloudSecret { name: String, secret_type: String },
    CloudBucket { bucket: String },
    CloudFile { path: String, is_dir: bool },
    AddSecretAction,
    AddBucketAction,
}

/// Formats the display line for a column in the database explorer tree.
fn format_column_display(
    indent: &str,
    name: &str,
    data_type: &str,
    is_pk: bool,
    is_fk: bool,
    fk_target: Option<&str>,
) -> String {
    if is_pk && is_fk {
        let target = fk_target.unwrap_or("unknown");
        format!("{}└─ 🔑🔗 {} ({}) [PK, FK -> {}]", indent, name, data_type, target)
    } else if is_pk {
        format!("{}└─ 🔑 {} ({}) [PK]", indent, name, data_type)
    } else if is_fk {
        let target = fk_target.unwrap_or("unknown");
        format!("{}└─ 🔗 {} ({}) [FK -> {}]", indent, name, data_type, target)
    } else {
        format!("{}└─ {} ({})", indent, name, data_type)
    }
}

/// State for the New Asset modal dialog (a or n in Project view).
#[derive(Clone, Debug)]
pub(crate) struct NewAssetModalState {
    pub(crate) is_open: bool,
    pub(crate) category_index: usize, // 0: Table, 1: View, 2: Macro, 3: Trigger, 4: Schema, 5: Query, 6: Seed
    pub(crate) name: String,
    pub(crate) active_field: usize, // 0: category, 1: name, 2: create, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl NewAssetModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            category_index: 0,
            name: String::new(),
            active_field: 1,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.category_index = 0;
        self.name.clear();
        self.active_field = 1;
        self.error_msg = None;
    }

    pub(crate) fn current_category(&self) -> ProjectCategory {
        match self.category_index {
            0 => ProjectCategory::Table,
            1 => ProjectCategory::View,
            2 => ProjectCategory::Macro,
            3 => ProjectCategory::Trigger,
            4 => ProjectCategory::Schema,
            5 => ProjectCategory::Query,
            _ => ProjectCategory::Seed,
        }
    }

    pub(crate) fn category_label(&self) -> &'static str {
        match self.category_index {
            0 => "Table",
            1 => "View",
            2 => "Macro",
            3 => "Trigger",
            4 => "Schema",
            5 => "Query",
            _ => "Seed",
        }
    }
}

/// State for the in-IDE Diff and Apply modal dialog (F6 or Ctrl+D).
#[derive(Clone, Debug)]
pub(crate) struct InIdeDiffModalState {
    pub(crate) is_open: bool,
    pub(crate) diff_result: Option<crate::diff::DiffResult>,
    pub(crate) selected_index: usize,
    pub(crate) error_msg: Option<String>,
}

impl InIdeDiffModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            diff_result: None,
            selected_index: 0,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = false;
        self.diff_result = None;
        self.selected_index = 0;
        self.error_msg = None;
    }
}

/// Generates starting DDL boilerplate for newly scaffolded project assets.
pub(crate) fn generate_asset_boilerplate(category: ProjectCategory, name: &str) -> String {
    let clean = name.trim().trim_end_matches(".sql").trim_end_matches(".csv");
    match category {
        ProjectCategory::Table => format!(
            "-- Table: {}\nCREATE TABLE {} (\n    id INTEGER PRIMARY KEY,\n    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP\n);\n",
            clean, clean
        ),
        ProjectCategory::View => format!(
            "-- View: {}\nCREATE VIEW {} AS\nSELECT 1 AS placeholder;\n",
            clean, clean
        ),
        ProjectCategory::Macro => format!(
            "-- Macro: {}\nCREATE MACRO {}(val) AS val;\n",
            clean, clean
        ),
        ProjectCategory::Trigger => format!(
            "-- Trigger: {}\nCREATE OR REPLACE TRIGGER {}\nAFTER INSERT ON <target_table>\nFOR EACH ROW\nEXECUTE FUNCTION <func_name>();\n",
            clean, clean
        ),
        ProjectCategory::Schema => format!(
            "-- Schema: {}\nCREATE SCHEMA IF NOT EXISTS {};\n",
            clean, clean
        ),
        ProjectCategory::Query => format!(
            "-- Query: {}\nSELECT 1;\n",
            clean
        ),
        ProjectCategory::Seed => format!(
            "id,name,value\n1,sample,100\n"
        ),
        _ => String::new(),
    }
}

/// Computes the disk file path for a project asset category and name.
/// Sanitizes path separators and `..` traversal components so the resulting path
/// always stays within the category directory under `project_dir`.
pub(crate) fn get_asset_file_path(project_dir: &Path, category: ProjectCategory, name: &str) -> PathBuf {
    let mut clean_name: String = name
        .trim()
        .chars()
        .filter(|&c| c != '/' && c != '\\' && c != '\0' && !c.is_control())
        .collect();
    while clean_name.contains("..") {
        clean_name = clean_name.replace("..", "_");
    }
    clean_name = clean_name.trim_start_matches('.').to_string();
    if clean_name.is_empty() {
        clean_name = "unnamed".to_string();
    }
    if category == ProjectCategory::Seed {
        if !clean_name.contains('.') {
            clean_name.push_str(".csv");
        }
    } else if !clean_name.ends_with(".sql") {
        clean_name.push_str(".sql");
    }
    let sub = match category {
        ProjectCategory::Schema => "schemas",
        ProjectCategory::Table => "tables",
        ProjectCategory::View => "views",
        ProjectCategory::Macro => "macros",
        ProjectCategory::Trigger => "triggers",
        ProjectCategory::Query => "queries",
        ProjectCategory::Seed => "seeds",
        ProjectCategory::Config => "",
    };
    if sub.is_empty() {
        project_dir.join(&clean_name)
    } else {
        project_dir.join(sub).join(&clean_name)
    }
}

/// Computes declarative diff between project definitions and the target database.
#[allow(dead_code)]
pub(crate) fn compute_ide_diff(
    project_dir: &Path,
    target_conn: &Connection,
) -> Result<crate::diff::DiffResult, String> {
    compute_ide_diff_for_catalog(project_dir, target_conn, None)
}

pub(crate) fn compute_ide_diff_for_catalog(
    project_dir: &Path,
    target_conn: &Connection,
    catalog: Option<&str>,
) -> Result<crate::diff::DiffResult, String> {
    let mut exec = target_conn;
    let target_cat = crate::diff::validate_target_catalog_exec(&mut exec, catalog, false).ok();
    crate::diff::diff_project(project_dir, &mut exec, target_cat.as_deref())
        .map_err(|e| format!("{:#}", e))
}

/// Applies declarative diff items to the target database catalog.
#[allow(dead_code)]
pub(crate) fn apply_ide_diff(
    project_dir: &Path,
    target_conn: &Connection,
    diff_result: &crate::diff::DiffResult,
) -> Result<(), String> {
    apply_ide_diff_for_catalog(project_dir, target_conn, diff_result, None)
}

pub(crate) fn apply_ide_diff_for_catalog(
    project_dir: &Path,
    target_conn: &Connection,
    diff_result: &crate::diff::DiffResult,
    catalog: Option<&str>,
) -> Result<(), String> {
    let mut exec = target_conn;
    let target_cat = crate::diff::validate_target_catalog_exec(&mut exec, catalog, true).ok();
    crate::engine::apply_diff_with_executor(
        &mut exec,
        diff_result,
        true,
        false,
        project_dir,
        target_cat.as_deref(),
    )
    .map_err(|e| format!("{:#}", e))
}

/// Passively detects the current Git branch and file status without modifying Git index.
pub(crate) fn get_git_branch_and_status(project_dir: &Path) -> (Option<String>, HashMap<PathBuf, String>) {
    let mut branch = None;
    let mut status_map = HashMap::new();

    if let Ok(output) = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(project_dir)
        .output()
    {
        if output.status.success() {
            let b = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !b.is_empty() {
                branch = Some(b);
            }
        }
    }

    if let Ok(output) = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_dir)
        .output()
    {
        if output.status.success() {
            let out_str = String::from_utf8_lossy(&output.stdout);
            for line in out_str.lines() {
                if line.len() >= 4 {
                    let code = &line[..2];
                    let file_part = line[3..].trim();
                    let file_path = project_dir.join(file_part);
                    let badge = if code.contains('M') {
                        "[M]".to_string()
                    } else if code.contains('?') || code.contains('A') {
                        "[+]".to_string()
                    } else if code.contains('D') {
                        "[D]".to_string()
                    } else {
                        format!("[{}]", code.trim())
                    };
                    status_map.insert(file_path, badge);
                }
            }
        }
    }

    (branch, status_map)
}

/// Scans the declarative project folder and builds a categorical asset hierarchy tree.
pub(crate) fn rebuild_project_nodes(
    project_dir: &Path,
    expanded_groups: &HashSet<String>,
    git_status: &HashMap<PathBuf, String>,
) -> Vec<ExplorerNode> {
    let mut items = Vec::new();

    let scan_dir = |sub: &str, ext: Option<&str>| -> Vec<PathBuf> {
        let dir = project_dir.join(sub);
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(expected_ext) = ext {
                        if path.extension().and_then(|s| s.to_str()) == Some(expected_ext) {
                            files.push(path);
                        }
                    } else {
                        files.push(path);
                    }
                }
            }
        }
        files.sort();
        files
    };

    // 1. Schemas
    let schemas = scan_dir("schemas", Some("sql"));
    let is_schemas_expanded = expanded_groups.contains("proj:schemas");
    items.push(ExplorerNode {
        name: "proj:schemas".to_string(),
        display: format!("📁 Schemas ({})", schemas.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_schemas_expanded,
        node_type: NodeType::Group,
    });
    if is_schemas_expanded {
        if schemas.is_empty() {
            items.push(ExplorerNode {
                name: "proj:schemas:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &schemas {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Schema },
                });
            }
        }
    }

    // 2. Tables
    let tables = scan_dir("tables", Some("sql"));
    let is_tables_expanded = expanded_groups.contains("proj:tables");
    items.push(ExplorerNode {
        name: "proj:tables".to_string(),
        display: format!("📦 Tables ({})", tables.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_tables_expanded,
        node_type: NodeType::Group,
    });
    if is_tables_expanded {
        if tables.is_empty() {
            items.push(ExplorerNode {
                name: "proj:tables:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &tables {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Table },
                });
            }
        }
    }

    // 3. Views
    let views = scan_dir("views", Some("sql"));
    let is_views_expanded = expanded_groups.contains("proj:views");
    items.push(ExplorerNode {
        name: "proj:views".to_string(),
        display: format!("👁️ Views ({})", views.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_views_expanded,
        node_type: NodeType::Group,
    });
    if is_views_expanded {
        if views.is_empty() {
            items.push(ExplorerNode {
                name: "proj:views:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &views {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::View },
                });
            }
        }
    }

    // 4. Macros
    let macros = scan_dir("macros", Some("sql"));
    let is_macros_expanded = expanded_groups.contains("proj:macros");
    items.push(ExplorerNode {
        name: "proj:macros".to_string(),
        display: format!("🔧 Macros ({})", macros.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_macros_expanded,
        node_type: NodeType::Group,
    });
    if is_macros_expanded {
        if macros.is_empty() {
            items.push(ExplorerNode {
                name: "proj:macros:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &macros {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Macro },
                });
            }
        }
    }

    // 5. Triggers
    let triggers = scan_dir("triggers", Some("sql"));
    let is_triggers_expanded = expanded_groups.contains("proj:triggers");
    items.push(ExplorerNode {
        name: "proj:triggers".to_string(),
        display: format!("⚡ Triggers ({})", triggers.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_triggers_expanded,
        node_type: NodeType::Group,
    });
    if is_triggers_expanded {
        if triggers.is_empty() {
            items.push(ExplorerNode {
                name: "proj:triggers:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &triggers {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Trigger },
                });
            }
        }
    }

    // 6. Queries
    let queries = scan_dir("queries", Some("sql"));
    let is_queries_expanded = expanded_groups.contains("proj:queries");
    items.push(ExplorerNode {
        name: "proj:queries".to_string(),
        display: format!("📄 Queries ({})", queries.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_queries_expanded,
        node_type: NodeType::Group,
    });
    if is_queries_expanded {
        if queries.is_empty() {
            items.push(ExplorerNode {
                name: "proj:queries:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &queries {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Query },
                });
            }
        }
    }

    // 7. Seeds
    let seeds = scan_dir("seeds", None);
    let is_seeds_expanded = expanded_groups.contains("proj:seeds");
    items.push(ExplorerNode {
        name: "proj:seeds".to_string(),
        display: format!("🌱 Seeds ({})", seeds.len()),
        level: 0,
        is_expandable: true,
        is_expanded: is_seeds_expanded,
        node_type: NodeType::Group,
    });
    if is_seeds_expanded {
        if seeds.is_empty() {
            items.push(ExplorerNode {
                name: "proj:seeds:empty".to_string(),
                display: "  └─ (empty - press 'a' to add)".to_string(),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Group,
            });
        } else {
            for p in &seeds {
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                items.push(ExplorerNode {
                    name: fname.clone(),
                    display: format!("  ├─ {}{}", fname, badge),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::ProjectFile { path: p.clone(), category: ProjectCategory::Seed },
                });
            }
        }
    }

    // 7. Config files
    let config_candidates = ["duckpack.toml", ".env", ".duckdbignore"];
    let mut configs = Vec::new();
    for c in &config_candidates {
        let p = project_dir.join(c);
        if p.exists() {
            configs.push(p);
        }
    }
    if !configs.is_empty() {
        for p in configs {
            let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let badge = git_status.get(&p).map(|b| format!(" {}", b)).unwrap_or_default();
            items.push(ExplorerNode {
                name: fname.clone(),
                display: format!("⚙️ {}{}", fname, badge),
                level: 0,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::ProjectFile { path: p, category: ProjectCategory::Config },
            });
        }
    }

    items
}

/// Supported database presets for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AttachProvider {
    DuckDB,
    Quack,
    MotherDuck,
    SQLite,
    PostgreSQL,
    MySQL,
    MSSQL,
}

impl AttachProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            AttachProvider::DuckDB => "DuckDB (Local / In-Memory)",
            AttachProvider::Quack => "Remote Quack Server",
            AttachProvider::MotherDuck => "MotherDuck Cloud",
            AttachProvider::SQLite => "SQLite",
            AttachProvider::PostgreSQL => "PostgreSQL",
            AttachProvider::MySQL => "MySQL",
            AttachProvider::MSSQL => "SQL Server / MSSQL (Community)",
        }
    }

    pub(crate) fn is_relational(&self) -> bool {
        matches!(self, AttachProvider::PostgreSQL | AttachProvider::MySQL | AttachProvider::MSSQL)
    }

    pub(crate) fn is_quack(&self) -> bool {
        matches!(self, AttachProvider::Quack)
    }

    pub(crate) fn is_motherduck(&self) -> bool {
        matches!(self, AttachProvider::MotherDuck)
    }

    #[allow(dead_code)]
    pub(crate) fn is_file_based(&self) -> bool {
        matches!(self, AttachProvider::DuckDB | AttachProvider::SQLite)
    }
}

/// State for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Debug)]
pub(crate) struct AttachModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: DuckDB, 1: Quack, 2: MotherDuck, 3: SQLite, 4: PostgreSQL, 5: MySQL, 6: MSSQL
    pub(crate) path: String,          // For DuckDB / SQLite file path, MotherDuck DB name
    pub(crate) host: String,          // Host / Server (localhost, etc.)
    pub(crate) port: String,          // Port (1433, 5432, 3306, 9494)
    pub(crate) database: String,      // Database name (mydb, postgres, etc.)
    pub(crate) username: String,      // Username / User ID (sa, postgres, root)
    pub(crate) password: String,      // Password / Auth Token (Masked)
    pub(crate) alias: String,         // Catalog alias
    pub(crate) alias_is_custom: bool,
    pub(crate) read_only: bool,
    pub(crate) disable_ssl: bool,
    pub(crate) insecure: bool,
    pub(crate) active_field: usize,
    pub(crate) error_msg: Option<String>,
}

impl AttachModalState {
    pub(crate) fn new() -> Self {
        let mut s = Self {
            is_open: false,
            provider_index: 0,
            path: String::new(),
            host: String::new(),
            port: String::new(),
            database: String::new(),
            username: String::new(),
            password: String::new(),
            alias: String::new(),
            alias_is_custom: false,
            read_only: false,
            disable_ssl: false,
            insecure: false,
            active_field: 1,
            error_msg: None,
        };
        s.set_provider(0);
        s
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.path.clear();
        self.host.clear();
        self.port.clear();
        self.database.clear();
        self.username.clear();
        self.password.clear();
        self.alias.clear();
        self.alias_is_custom = false;
        self.read_only = false;
        self.disable_ssl = false;
        self.insecure = false;
        self.active_field = 1;
        self.error_msg = None;
        self.set_provider(0);
    }

    pub(crate) fn provider(&self) -> AttachProvider {
        match self.provider_index {
            1 => AttachProvider::Quack,
            2 => AttachProvider::MotherDuck,
            3 => AttachProvider::SQLite,
            4 => AttachProvider::PostgreSQL,
            5 => AttachProvider::MySQL,
            6 => AttachProvider::MSSQL,
            _ => AttachProvider::DuckDB,
        }
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 7;
        self.error_msg = None;
        let is_default_port = matches!(self.port.as_str(), "" | "1433" | "5432" | "3306" | "9494");
        let is_default_user = matches!(self.username.as_str(), "" | "sa" | "postgres" | "root");

        match self.provider() {
            AttachProvider::MSSQL => {
                if self.host.is_empty() { self.host = "localhost".to_string(); }
                if is_default_port { self.port = "1433".to_string(); }
                if is_default_user { self.username = "sa".to_string(); }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() { self.database.clone() } else { "mssql_db".to_string() };
                }
            }
            AttachProvider::PostgreSQL => {
                if self.host.is_empty() { self.host = "localhost".to_string(); }
                if is_default_port { self.port = "5432".to_string(); }
                if is_default_user { self.username = "postgres".to_string(); }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() { self.database.clone() } else { "pg_db".to_string() };
                }
            }
            AttachProvider::MySQL => {
                if self.host.is_empty() { self.host = "localhost".to_string(); }
                if is_default_port { self.port = "3306".to_string(); }
                if is_default_user { self.username = "root".to_string(); }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() { self.database.clone() } else { "mysql_db".to_string() };
                }
            }
            AttachProvider::Quack => {
                if self.host.is_empty() { self.host = "localhost".to_string(); }
                if is_default_port { self.port = "9494".to_string(); }
                if !self.alias_is_custom {
                    self.alias = "quack_remote".to_string();
                }
            }
            AttachProvider::MotherDuck => {
                if !self.alias_is_custom {
                    self.alias = if !self.path.is_empty() { self.path.trim_start_matches("md:").to_string() } else { "my_db".to_string() };
                }
            }
            AttachProvider::SQLite => {
                if !self.alias_is_custom {
                    self.alias = "sqlite_db".to_string();
                }
            }
            AttachProvider::DuckDB => {
                if !self.alias_is_custom {
                    self.alias = "attached_db".to_string();
                }
            }
        }
        let total = self.field_count();
        if self.active_field >= total {
            self.active_field = 1;
        }
    }

    pub(crate) fn field_count(&self) -> usize {
        let p = self.provider();
        if p.is_relational() {
            12
        } else if p.is_quack() {
            10
        } else if p.is_motherduck() {
            7
        } else {
            6
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_submit(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            self.active_field == 10
        } else if p.is_quack() {
            self.active_field == 8
        } else if p.is_motherduck() {
            self.active_field == 5
        } else {
            self.active_field == 4
        }
    }

    pub(crate) fn is_cancel(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            self.active_field == 11
        } else if p.is_quack() {
            self.active_field == 9
        } else if p.is_motherduck() {
            self.active_field == 6
        } else {
            self.active_field == 5
        }
    }

    pub(crate) fn is_toggle_field(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            (7..=9).contains(&self.active_field)
        } else if p.is_quack() {
            (5..=7).contains(&self.active_field)
        } else if p.is_motherduck() {
            self.active_field == 4
        } else {
            self.active_field == 3
        }
    }

    pub(crate) fn toggle_active(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                7 => self.read_only = !self.read_only,
                8 => self.disable_ssl = !self.disable_ssl,
                9 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                5 => self.read_only = !self.read_only,
                6 => self.disable_ssl = !self.disable_ssl,
                7 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_motherduck() {
            if self.active_field == 4 {
                self.read_only = !self.read_only;
            }
        } else if self.active_field == 3 {
            self.read_only = !self.read_only;
        }
    }

    pub(crate) fn next_field(&mut self) {
        let total = self.field_count();
        self.active_field = (self.active_field + 1) % total;
    }

    pub(crate) fn prev_field(&mut self) {
        let total = self.field_count();
        self.active_field = if self.active_field == 0 { total - 1 } else { self.active_field - 1 };
    }

    pub(crate) fn nav_left(&mut self) {
        let p = self.provider();
        if self.active_field == 0 {
            let prev = if self.provider_index == 0 { 6 } else { self.provider_index - 1 };
            self.set_provider(prev);
        } else if p.is_relational() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                7 => self.active_field = 9,
                8 => self.active_field = 7,
                9 => self.active_field = 8,
                10 => self.active_field = 11,
                11 => self.active_field = 10,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 7,
                6 => self.active_field = 5,
                7 => self.active_field = 6,
                8 => self.active_field = 9,
                9 => self.active_field = 8,
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                2 => self.active_field = 3,
                3 => self.active_field = 2,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                _ => {}
            }
        } else {
            match self.active_field {
                4 => self.active_field = 5,
                5 => self.active_field = 4,
                _ => {}
            }
        }
    }

    pub(crate) fn nav_right(&mut self) {
        let p = self.provider();
        if self.active_field == 0 {
            let next = (self.provider_index + 1) % 7;
            self.set_provider(next);
        } else if p.is_relational() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                7 => self.active_field = 8,
                8 => self.active_field = 9,
                9 => self.active_field = 7,
                10 => self.active_field = 11,
                11 => self.active_field = 10,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 7,
                7 => self.active_field = 5,
                8 => self.active_field = 9,
                9 => self.active_field = 8,
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                2 => self.active_field = 3,
                3 => self.active_field = 2,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                _ => {}
            }
        } else {
            match self.active_field {
                4 => self.active_field = 5,
                5 => self.active_field = 4,
                _ => {}
            }
        }
    }

    pub(crate) fn nav_down(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5 | 6 => 7,
                7..=9 => 10,
                10 | 11 => 0,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5..=7 => 8,
                8 | 9 => 0,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 | 3 => 4,
                4 => 5,
                5 | 6 => 0,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 => 3,
                3 => 4,
                4 | 5 => 0,
                _ => 0,
            };
        }
    }

    pub(crate) fn nav_up(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 10,
                1 | 2 => 0,
                3 | 4 => 1,
                5 | 6 => 3,
                7..=9 => 5,
                10 | 11 => 7,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 8,
                1 | 2 => 0,
                3 | 4 => 1,
                5..=7 => 3,
                8 | 9 => 5,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 5,
                1 => 0,
                2 | 3 => 1,
                4 => 2,
                5 | 6 => 4,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 4,
                1 => 0,
                2 => 1,
                3 => 2,
                4 | 5 => 3,
                _ => 0,
            };
        }
    }

    pub(crate) fn handle_char(&mut self, c: char) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                1 => self.host.push(c),
                2 => if c.is_ascii_digit() { self.port.push(c); },
                3 => {
                    self.database.push(c);
                    if !self.alias_is_custom {
                        self.alias = self.database.clone();
                    }
                }
                4 => self.username.push(c),
                5 => self.password.push(c),
                6 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.host.push(c),
                2 => if c.is_ascii_digit() { self.port.push(c); },
                3 => self.password.push(c),
                4 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => {
                    self.path.push(c);
                    if !self.alias_is_custom {
                        self.alias = self.path.trim_start_matches("md:").to_string();
                    }
                }
                2 => self.password.push(c),
                3 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else {
            match self.active_field {
                1 => {
                    self.path.push(c);
                    if !self.alias_is_custom {
                        let stem = Path::new(&self.path).file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        self.alias = stem.to_string();
                    }
                }
                2 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn handle_backspace(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                1 => { self.host.pop(); }
                2 => { self.port.pop(); }
                3 => {
                    self.database.pop();
                    if !self.alias_is_custom {
                        self.alias = self.database.clone();
                    }
                }
                4 => { self.username.pop(); }
                5 => { self.password.pop(); }
                6 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => { self.host.pop(); }
                2 => { self.port.pop(); }
                3 => { self.password.pop(); }
                4 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => {
                    self.path.pop();
                    if !self.alias_is_custom {
                        self.alias = self.path.trim_start_matches("md:").to_string();
                    }
                }
                2 => { self.password.pop(); }
                3 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else {
            match self.active_field {
                1 => {
                    self.path.pop();
                    if !self.alias_is_custom {
                        let stem = Path::new(&self.path).file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        self.alias = stem.to_string();
                    }
                }
                2 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn build_connection_target(&self) -> std::result::Result<(String, String), String> {
        let provider = self.provider();
        match provider {
            AttachProvider::MSSQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MSSQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut conn_str = if host_raw.contains('=') {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() { "1433" } else { self.port.trim() };
                    format!("Server={},{}", host_raw, port)
                };
                if !self.database.trim().is_empty() && !conn_str.to_lowercase().contains("database=") {
                    conn_str.push_str(&format!(";Database={}", self.database.trim()));
                }
                if !self.username.trim().is_empty() && !conn_str.to_lowercase().contains("user id=") && !conn_str.to_lowercase().contains("uid=") {
                    conn_str.push_str(&format!(";User Id={}", self.username.trim()));
                }
                if !self.password.trim().is_empty() && !conn_str.to_lowercase().contains("password=") && !conn_str.to_lowercase().contains("pwd=") {
                    conn_str.push_str(&format!(";Password={}", self.password.trim()));
                }
                if (self.insecure || conn_str.contains("localhost") || conn_str.contains("127.0.0.1"))
                    && !conn_str.to_lowercase().contains("trustservercertificate=")
                {
                    conn_str.push_str(";TrustServerCertificate=yes");
                }
                if self.disable_ssl && !conn_str.to_lowercase().contains("encrypt=") {
                    conn_str.push_str(";Encrypt=no");
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() { self.database.trim().to_string() } else { "mssql_db".to_string() }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((conn_str.replace('\'', "''"), alias))
            }
            AttachProvider::PostgreSQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for PostgreSQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() { "5432" } else { self.port.trim() };
                    let user_part = if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                        format!("{}:{}@", self.username.trim(), self.password.trim())
                    } else if !self.username.trim().is_empty() {
                        format!("{}@", self.username.trim())
                    } else {
                        String::new()
                    };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("postgres://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") { uri.push_str("&sslmode=disable"); }
                    } else {
                        uri.push_str("?sslmode=disable");
                    }
                } else if self.insecure {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") { uri.push_str("&sslmode=prefer"); }
                    } else {
                        uri.push_str("?sslmode=prefer");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() { self.database.trim().to_string() } else { "pg_db".to_string() }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::MySQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MySQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() { "3306" } else { self.port.trim() };
                    let user_part = if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                        format!("{}:{}@", self.username.trim(), self.password.trim())
                    } else if !self.username.trim().is_empty() {
                        format!("{}@", self.username.trim())
                    } else {
                        String::new()
                    };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("mysql://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("ssl-mode=") { uri.push_str("&ssl-mode=DISABLED"); }
                    } else {
                        uri.push_str("?ssl-mode=DISABLED");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() { self.database.trim().to_string() } else { "mysql_db".to_string() }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::Quack => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server cannot be empty for Remote Quack.".to_string());
                }
                let host_raw = self.host.trim();
                let stripped = host_raw.strip_prefix("quack://").or_else(|| host_raw.strip_prefix("quack:")).unwrap_or(host_raw);
                let endpoint = if stripped.contains(':') {
                    stripped.to_string()
                } else {
                    let port = if self.port.trim().is_empty() { "9494" } else { self.port.trim() };
                    format!("{}:{}", stripped, port)
                };
                let target = format!("quack:{}", endpoint);
                let alias = if self.alias.trim().is_empty() { "quack_remote".to_string() } else { self.alias.trim().to_string() };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::MotherDuck => {
                if self.path.trim().is_empty() {
                    return Err("MotherDuck database name cannot be empty.".to_string());
                }
                let target = if self.path.trim().starts_with("md:") {
                    self.path.trim().to_string()
                } else {
                    format!("md:{}", self.path.trim())
                };
                let alias = if self.alias.trim().is_empty() {
                    self.path.trim().trim_start_matches("md:").to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::SQLite | AttachProvider::DuckDB => {
                if self.path.trim().is_empty() {
                    return Err("Database file path cannot be empty.".to_string());
                }
                let alias = if self.alias.trim().is_empty() {
                    Path::new(self.path.trim())
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("attached_db")
                        .to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((self.path.trim().replace('\'', "''"), alias))
            }
        }
    }
}

/// Supported cloud storage providers for secrets and bucket exploration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CloudProvider {
    S3,
    CloudflareR2,
    MinIO,
    Wasabi,
    CustomS3,
}

impl CloudProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            CloudProvider::S3 => "AWS S3",
            CloudProvider::CloudflareR2 => "Cloudflare R2",
            CloudProvider::MinIO => "MinIO (Self-hosted)",
            CloudProvider::Wasabi => "Wasabi Cloud",
            CloudProvider::CustomS3 => "Custom S3 Endpoint",
        }
    }
}

/// State for the Cloud Secrets & S3 modal dialog (Ctrl+K).
#[derive(Clone, Debug)]
pub(crate) struct SecretModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: S3, 1: R2, 2: MinIO, 3: Wasabi, 4: Custom
    pub(crate) name: String,
    pub(crate) key_id: String,
    pub(crate) secret_key: String,
    pub(crate) region: String,
    pub(crate) endpoint: String,
    pub(crate) bucket: String,
    pub(crate) use_ssl: bool,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: provider, 1: name, 2: key_id, 3: secret_key, 4: region, 5: endpoint, 6: bucket, 7: use_ssl, 8: save_to_env, 9: submit, 10: cancel
    pub(crate) error_msg: Option<String>,
}

impl SecretModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            provider_index: 0,
            name: "s3_secret".to_string(),
            key_id: String::new(),
            secret_key: String::new(),
            region: "us-east-1".to_string(),
            endpoint: String::new(),
            bucket: String::new(),
            use_ssl: true,
            save_to_env: true,
            active_field: 2,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.name = "s3_secret".to_string();
        self.key_id.clear();
        self.secret_key.clear();
        self.region = "us-east-1".to_string();
        self.endpoint.clear();
        self.bucket.clear();
        self.use_ssl = true;
        self.save_to_env = true;
        self.active_field = 2;
        self.error_msg = None;
    }

    pub(crate) fn provider(&self) -> CloudProvider {
        match self.provider_index {
            1 => CloudProvider::CloudflareR2,
            2 => CloudProvider::MinIO,
            3 => CloudProvider::Wasabi,
            4 => CloudProvider::CustomS3,
            _ => CloudProvider::S3,
        }
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 5;
        match self.provider() {
            CloudProvider::S3 => {
                if self.endpoint == "localhost:9000" || self.endpoint == "s3.wasabisys.com" || self.endpoint.contains(".r2.cloudflarestorage.com") {
                    self.endpoint.clear();
                }
                if self.region == "auto" {
                    self.region = "us-east-1".to_string();
                }
                self.use_ssl = true;
            }
            CloudProvider::CloudflareR2 => {
                if self.endpoint.is_empty() || self.endpoint == "localhost:9000" || self.endpoint == "s3.wasabisys.com" {
                    self.endpoint = "<account_id>.r2.cloudflarestorage.com".to_string();
                }
                self.region = "auto".to_string();
                self.use_ssl = true;
            }
            CloudProvider::MinIO => {
                self.endpoint = "localhost:9000".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = false;
            }
            CloudProvider::Wasabi => {
                self.endpoint = "s3.wasabisys.com".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = true;
            }
            CloudProvider::CustomS3 => {
                self.use_ssl = true;
            }
        }
    }
}

/// State for the Add Cloud Bucket modal dialog.
#[derive(Clone, Debug)]
pub(crate) struct AddBucketModalState {
    pub(crate) is_open: bool,
    pub(crate) initial_uri: Option<String>,
    pub(crate) uri: String,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: uri, 1: save_to_env, 2: submit, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl AddBucketModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            initial_uri: None,
            uri: String::new(),
            save_to_env: true,
            active_field: 0,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.initial_uri = None;
        self.uri.clear();
        self.save_to_env = true;
        self.active_field = 0;
        self.error_msg = None;
    }
}

/// State for the Database Picker modal dialog (F2).
#[derive(Clone, Debug)]
pub(crate) struct DatabasePickerModalState {
    pub(crate) is_open: bool,
    pub(crate) selected_index: usize,
}

impl DatabasePickerModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            selected_index: 0,
        }
    }

    pub(crate) fn open_for_current(&mut self, current_db: &str, databases: &[crate::catalog::DatabaseInfo]) {
        self.is_open = true;
        if let Some(pos) = databases.iter().position(|d| d.name == current_db) {
            self.selected_index = pos;
        } else {
            self.selected_index = 0;
        }
    }

    pub(crate) fn next(&mut self, total: usize) {
        if total > 0 {
            self.selected_index = (self.selected_index + 1).min(total.saturating_sub(1));
        }
    }

    pub(crate) fn prev(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }
}

/// State for the Exit Confirmation modal dialog (Esc).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExitConfirmationModalState {
    pub(crate) is_open: bool,
    pub(crate) confirm_exit: bool,
}

impl ExitConfirmationModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            confirm_exit: false,
        }
    }

    pub(crate) fn open(&mut self) {
        self.is_open = true;
        self.confirm_exit = false; // default to safe cancel
    }

    pub(crate) fn close(&mut self) {
        self.is_open = false;
        self.confirm_exit = false;
    }

    pub(crate) fn toggle(&mut self) {
        self.confirm_exit = !self.confirm_exit;
    }
}

/// Populates SecretModalState with existing secret configuration from DuckDB and .env for in-place editing.
pub(crate) fn populate_secret_modal_for_edit(
    conn: &duckdb::Connection,
    project_dir: &std::path::Path,
    secret_name: &str,
    modal: &mut SecretModalState,
) {
    modal.is_open = true;
    modal.name = secret_name.to_string();
    modal.active_field = 2; // Focus on Key ID by default
    modal.error_msg = None;

    // 1. Query DuckDB's duckdb_secrets() to extract existing config
    let sql = format!("SELECT secret_string FROM duckdb_secrets() WHERE name = '{}';", secret_name.replace('\'', "''"));
    let mut secret_string_opt = None;
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(mut rows) = stmt.query([]) {
            if let Ok(Some(row)) = rows.next() {
                secret_string_opt = row.get::<_, String>(0).ok();
            }
        }
    }
    if secret_string_opt.is_none() {
        let fallback_sql = format!("SELECT secret_string FROM duckdb_secrets WHERE name = '{}';", secret_name.replace('\'', "''"));
        if let Ok(mut stmt) = conn.prepare(&fallback_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                if let Ok(Some(row)) = rows.next() {
                    secret_string_opt = row.get::<_, String>(0).ok();
                }
            }
        }
    }
    if let Some(secret_string) = secret_string_opt {
        for part in secret_string.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                match k.trim() {
                    "key_id" => modal.key_id = v.trim().to_string(),
                    "region" => modal.region = v.trim().to_string(),
                    "endpoint" => modal.endpoint = v.trim().to_string(),
                    "use_ssl" => modal.use_ssl = v.trim().eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }
    }

    // 2. Read .env file for secrets / bucket if available
    let env_path = project_dir.join(".env");
    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"').trim_matches('\'');
                match key {
                    "AWS_ACCESS_KEY_ID" if modal.key_id.is_empty() => modal.key_id = val.to_string(),
                    "AWS_SECRET_ACCESS_KEY" => modal.secret_key = val.to_string(),
                    "AWS_REGION" if modal.region.is_empty() => modal.region = val.to_string(),
                    "AWS_ENDPOINT" if modal.endpoint.is_empty() => modal.endpoint = val.to_string(),
                    "DUCKPACK_S3_BUCKET" if modal.bucket.is_empty() => modal.bucket = val.to_string(),
                    _ => {}
                }
            }
        }
    }

    // 3. Infer provider index from endpoint
    let ep_lower = modal.endpoint.to_lowercase();
    if ep_lower.contains("r2.cloudflarestorage.com") {
        modal.provider_index = 1;
    } else if ep_lower.contains("localhost:9000") || ep_lower.contains("127.0.0.1:9000") || ep_lower.contains("minio") || ep_lower.contains("rustfs") {
        modal.provider_index = 2;
    } else if ep_lower.contains("wasabisys.com") {
        modal.provider_index = 3;
    } else if !modal.endpoint.is_empty() {
        modal.provider_index = 4;
    } else {
        modal.provider_index = 0;
    }
}

/// Represents a single renderable node in the Explorer sidebar.
/// Tracks its hierarchical depth (`level`) and expandability state for collapsing/expanding tables.
#[derive(Clone, Debug)]
pub(crate) struct ExplorerNode {
    pub(crate) name: String,
    pub(crate) display: String,
    pub(crate) level: usize,
    pub(crate) is_expandable: bool,
    pub(crate) is_expanded: bool,
    pub(crate) node_type: NodeType,
}

/// Represents an individual open Query Tab within the IDE.
/// Contains an isolated text area and its own dedicated query results and scrolling state.
struct EditorTab<'a> {
    textarea: TextArea<'a>,
    active_file: Option<PathBuf>,
    is_dirty: bool,
    query_results: Vec<Vec<String>>,
    column_names: Vec<String>,
    horizontal_scroll: usize,
    results_state: ratatui::widgets::TableState,
    base_query: Option<String>,
    pagination_offset: usize,
    pagination_limit: usize,
    is_paginated: bool,
    target_database: String,
}

impl<'a> EditorTab<'a> {
    fn new(target_database: String) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
        textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
        Self {
            textarea,
            active_file: None,
            is_dirty: false,
            query_results: Vec::new(),
            column_names: Vec::new(),
            horizontal_scroll: 0,
            results_state: ratatui::widgets::TableState::default(),
            base_query: None,
            pagination_offset: 0,
            pagination_limit: 500,
            is_paginated: false,
            target_database,
        }
    }
}

/// Formats the database provider icon based on database type.
pub(crate) fn db_icon(db_type: &str) -> &'static str {
    match db_type.to_lowercase().as_str() {
        "sqlite" => "🗃️",
        "postgres" | "postgresql" | "mysql" | "mssql" => "🔌",
        _ => "📁",
    }
}

pub(crate) fn rebuild_explorer_nodes(
    meta: &ExplorerMetadata,
    expanded_groups: &HashSet<String>,
    expanded_tables: &HashSet<String>,
    conn: &Connection,
    is_quack: bool,
) -> Vec<ExplorerNode> {
    let mut items = Vec::new();

    if meta.databases.len() > 1 {
        // Multi-database mode: render database root nodes
        for db in &meta.databases {
            let db_key = format!("db:{}", db.name);
            let is_db_expanded = expanded_groups.contains(&db_key);
            let icon = db_icon(&db.db_type);
            let active_tag = if db.is_active { " [Active]" } else { "" };
            let ro_tag = if db.readonly { " [RO]" } else { "" };
            let type_tag = format!(" ({})", db.db_type);

            items.push(ExplorerNode {
                name: db_key.clone(),
                display: format!("{} {}{}{}{}", icon, db.name, active_tag, type_tag, ro_tag),
                level: 0,
                is_expandable: true,
                is_expanded: is_db_expanded,
                node_type: NodeType::Database {
                    db_type: db.db_type.clone(),
                    is_active: db.is_active,
                    path: db.path.clone(),
                },
            });

            if is_db_expanded {
                if !db.tables.is_empty() {
                    let tbl_group_key = format!("{}:Tables", db.name);
                    let is_tbl_group_expanded = expanded_groups.contains(&tbl_group_key);
                    items.push(ExplorerNode {
                        name: tbl_group_key.clone(),
                        display: format!("  📦 Tables ({})", db.tables.len()),
                        level: 1,
                        is_expandable: true,
                        is_expanded: is_tbl_group_expanded,
                        node_type: NodeType::Group,
                    });

                    if is_tbl_group_expanded {
                        for t in &db.tables {
                            let table_key = t.qualified_name();
                            let is_tbl_expanded = expanded_tables.contains(&table_key) || expanded_tables.contains(&t.display_name()) || expanded_tables.contains(&t.name);
                            let count_str = t.row_count.map(|c| format!(" ({})", format_row_count(c))).unwrap_or_default();
                            let temp_tag = if t.is_temporary { " [temp]" } else { "" };
                            items.push(ExplorerNode {
                                name: t.name.clone(),
                                display: format!("    ├─ {}{}{}", t.display_name(), temp_tag, count_str),
                                level: 2,
                                is_expandable: true,
                                is_expanded: is_tbl_expanded,
                                node_type: NodeType::Table {
                                    database: Some(db.name.clone()),
                                    schema: Some(t.schema.clone()),
                                    is_temporary: t.is_temporary,
                                },
                            });

                            if is_tbl_expanded {
                                let cols = load_table_columns_for(&db.name, &t.schema, &t.name, conn, is_quack);
                                for col in cols {
                                    let display = format_column_display("      ", &col.name, &col.data_type, col.is_pk, col.is_fk, col.fk_target.as_deref());
                                    items.push(ExplorerNode {
                                        name: col.name.clone(),
                                        display,
                                        level: 3,
                                        is_expandable: false,
                                        is_expanded: false,
                                        node_type: NodeType::Column {
                                            data_type: col.data_type,
                                            is_pk: col.is_pk,
                                            is_fk: col.is_fk,
                                            fk_target: col.fk_target,
                                        },
                                    });
                                }
                            }
                        }
                    }
                }

                if !db.views.is_empty() {
                    let view_group_key = format!("{}:Views", db.name);
                    let is_view_group_expanded = expanded_groups.contains(&view_group_key);
                    items.push(ExplorerNode {
                        name: view_group_key.clone(),
                        display: format!("  👁️ Views ({})", db.views.len()),
                        level: 1,
                        is_expandable: true,
                        is_expanded: is_view_group_expanded,
                        node_type: NodeType::Group,
                    });

                    if is_view_group_expanded {
                        for v in &db.views {
                            let view_key = v.qualified_name();
                            let is_view_expanded = expanded_tables.contains(&view_key) || expanded_tables.contains(&v.display_name()) || expanded_tables.contains(&v.name);
                            let temp_tag = if v.is_temporary { " [temp]" } else { "" };
                            items.push(ExplorerNode {
                                name: v.name.clone(),
                                display: format!("    ├─ {}{}", v.display_name(), temp_tag),
                                level: 2,
                                is_expandable: true,
                                is_expanded: is_view_expanded,
                                node_type: NodeType::View {
                                    database: Some(db.name.clone()),
                                    schema: Some(v.schema.clone()),
                                    is_temporary: v.is_temporary,
                                },
                            });

                            if is_view_expanded {
                                let cols = load_table_columns_for(&db.name, &v.schema, &v.name, conn, is_quack);
                                for col in cols {
                                    items.push(ExplorerNode {
                                        name: col.name.clone(),
                                        display: format!("      └─ {} ({})", col.name, col.data_type),
                                        level: 3,
                                        is_expandable: false,
                                        is_expanded: false,
                                        node_type: NodeType::Column {
                                            data_type: col.data_type,
                                            is_pk: false,
                                            is_fk: false,
                                            fk_target: None,
                                        },
                                    });
                                }
                            }
                        }
                    }
                }

                if !db.triggers.is_empty() {
                    let trg_group_key = format!("{}:Triggers", db.name);
                    let is_trg_group_expanded = expanded_groups.contains(&trg_group_key);
                    items.push(ExplorerNode {
                        name: trg_group_key.clone(),
                        display: format!("  ⚡ Triggers ({})", db.triggers.len()),
                        level: 1,
                        is_expandable: true,
                        is_expanded: is_trg_group_expanded,
                        node_type: NodeType::Group,
                    });

                    if is_trg_group_expanded {
                        for trg in &db.triggers {
                            items.push(ExplorerNode {
                                name: trg.name.clone(),
                                display: format!("    ├─ {} (on {})", trg.display_name(), trg.table_name),
                                level: 2,
                                is_expandable: false,
                                is_expanded: false,
                                node_type: NodeType::Trigger {
                                    database: Some(db.name.clone()),
                                    schema: Some(trg.schema.clone()),
                                    table: Some(trg.table_name.clone()),
                                },
                            });
                        }
                    }
                }
            }
        }
    } else {
        // Single database mode (classic flat layout)
        if !meta.tables.is_empty() {
            let is_expanded = expanded_groups.contains("Tables");
            items.push(ExplorerNode {
                name: "Tables".to_string(),
                display: format!("📦 Tables ({})", meta.tables.len()),
                level: 0,
                is_expandable: true,
                is_expanded,
                node_type: NodeType::Group,
            });
            if is_expanded {
                for t in &meta.tables {
                    let is_tbl_expanded = expanded_tables.contains(t);
                    let is_temp = meta.temp_tables.contains(t);
                    let temp_tag = if is_temp { " [temp]" } else { "" };
                    let count_str = meta.table_counts.get(t).map(|&c| format!(" ({})", format_row_count(c))).unwrap_or_default();
                    items.push(ExplorerNode {
                        name: t.clone(),
                        display: format!("  ├─ {}{}{}", t, temp_tag, count_str),
                        level: 1,
                        is_expandable: true,
                        is_expanded: is_tbl_expanded,
                        node_type: NodeType::Table {
                            database: Some(meta.active_database.clone()),
                            schema: Some("main".to_string()),
                            is_temporary: is_temp,
                        },
                    });
                    if is_tbl_expanded {
                        let cols = load_table_columns_for(&meta.active_database, "main", t, conn, is_quack);
                        for col in cols {
                            let display = format_column_display("    ", &col.name, &col.data_type, col.is_pk, col.is_fk, col.fk_target.as_deref());
                            items.push(ExplorerNode {
                                name: col.name.clone(),
                                display,
                                level: 2,
                                is_expandable: false,
                                is_expanded: false,
                                node_type: NodeType::Column {
                                    data_type: col.data_type,
                                    is_pk: col.is_pk,
                                    is_fk: col.is_fk,
                                    fk_target: col.fk_target,
                                },
                            });
                        }
                    }
                }
            }
        }

        if !meta.views.is_empty() {
            let is_expanded = expanded_groups.contains("Views");
            items.push(ExplorerNode {
                name: "Views".to_string(),
                display: format!("👁️ Views ({})", meta.views.len()),
                level: 0,
                is_expandable: true,
                is_expanded,
                node_type: NodeType::Group,
            });
            if is_expanded {
                for v in &meta.views {
                    let is_view_expanded = expanded_tables.contains(v);
                    let is_temp = meta.temp_views.contains(v);
                    let temp_tag = if is_temp { " [temp]" } else { "" };
                    items.push(ExplorerNode {
                        name: v.clone(),
                        display: format!("  ├─ {}{}", v, temp_tag),
                        level: 1,
                        is_expandable: true,
                        is_expanded: is_view_expanded,
                        node_type: NodeType::View {
                            database: Some(meta.active_database.clone()),
                            schema: Some("main".to_string()),
                            is_temporary: is_temp,
                        },
                    });
                    if is_view_expanded {
                        let cols = load_table_columns_for(&meta.active_database, "main", v, conn, is_quack);
                        for col in cols {
                            items.push(ExplorerNode {
                                name: col.name.clone(),
                                display: format!("    └─ {} ({})", col.name, col.data_type),
                                level: 2,
                                is_expandable: false,
                                is_expanded: false,
                                node_type: NodeType::Column {
                                    data_type: col.data_type,
                                    is_pk: false,
                                    is_fk: false,
                                    fk_target: None,
                                },
                            });
                        }
                    }
                }
            }
        }

        if !meta.triggers.is_empty() {
            let is_expanded = expanded_groups.contains("Triggers");
            items.push(ExplorerNode {
                name: "Triggers".to_string(),
                display: format!("⚡ Triggers ({})", meta.triggers.len()),
                level: 0,
                is_expandable: true,
                is_expanded,
                node_type: NodeType::Group,
            });
            if is_expanded {
                for trg in &meta.triggers {
                    items.push(ExplorerNode {
                        name: trg.clone(),
                        display: format!("  ├─ {}", trg),
                        level: 1,
                        is_expandable: false,
                        is_expanded: false,
                        node_type: NodeType::Trigger {
                            database: Some(meta.active_database.clone()),
                            schema: Some("main".to_string()),
                            table: None,
                        },
                    });
                }
            }
        }
    }

    if !meta.macros.is_empty() {
        let is_expanded = expanded_groups.contains("Macros");
        items.push(ExplorerNode {
            name: "Macros".to_string(),
            display: format!("🔧 Macros ({})", meta.macros.len()),
            level: 0,
            is_expandable: true,
            is_expanded,
            node_type: NodeType::Group,
        });
        if is_expanded {
            for m in &meta.macros {
                items.push(ExplorerNode {
                    name: m.clone(),
                    display: format!("  ├─ {}", m),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::Macro,
                });
            }
        }
    }

    if !meta.queries.is_empty() {
        let is_expanded = expanded_groups.contains("Queries");
        items.push(ExplorerNode {
            name: "Queries".to_string(),
            display: format!("📄 Queries ({})", meta.queries.len()),
            level: 0,
            is_expandable: true,
            is_expanded,
            node_type: NodeType::Group,
        });
        if is_expanded {
            for q in &meta.queries {
                items.push(ExplorerNode {
                    name: q.clone(),
                    display: format!("  ├─ {}", q),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::Query,
                });
            }
        }
    }

    // Secrets & Cloud Buckets Section
    let is_secrets_expanded = expanded_groups.contains("Secrets & Cloud Buckets");
    let total_secrets = meta.secrets.len() + meta.cloud_buckets.len();
    items.push(ExplorerNode {
        name: "Secrets & Cloud Buckets".to_string(),
        display: format!("🔐 Secrets & Cloud ({})", total_secrets),
        level: 0,
        is_expandable: true,
        is_expanded: is_secrets_expanded,
        node_type: NodeType::Group,
    });
    if is_secrets_expanded {
        items.push(ExplorerNode {
            name: "add_cloud_secret".to_string(),
            display: "  ➕ Add Cloud Secret (Ctrl+K)".to_string(),
            level: 1,
            is_expandable: false,
            is_expanded: false,
            node_type: NodeType::AddSecretAction,
        });

        items.push(ExplorerNode {
            name: "add_cloud_bucket".to_string(),
            display: "  ➕ Add S3 Bucket (Enter)".to_string(),
            level: 1,
            is_expandable: false,
            is_expanded: false,
            node_type: NodeType::AddBucketAction,
        });

        for (sec_name, sec_type) in &meta.secrets {
            items.push(ExplorerNode {
                name: sec_name.clone(),
                display: format!("  🔑 {} [{}]", sec_name, sec_type),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::CloudSecret {
                    name: sec_name.clone(),
                    secret_type: sec_type.clone(),
                },
            });
        }

        for bucket in &meta.cloud_buckets {
            let bucket_key = format!("bucket:{}", bucket);
            let is_bucket_expanded = expanded_groups.contains(&bucket_key);
            items.push(ExplorerNode {
                name: bucket_key.clone(),
                display: format!("  🪣 {}", bucket),
                level: 1,
                is_expandable: true,
                is_expanded: is_bucket_expanded,
                node_type: NodeType::CloudBucket { bucket: bucket.clone() },
            });

            if is_bucket_expanded {
                match crate::catalog::list_s3_bucket_objects(conn, bucket) {
                    Ok(objects) if objects.is_empty() => {
                        items.push(ExplorerNode {
                            name: format!("{}:empty", bucket),
                            display: "    (bucket is empty)".to_string(),
                            level: 2,
                            is_expandable: false,
                            is_expanded: false,
                            node_type: NodeType::Group,
                        });
                    }
                    Ok(objects) => {
                        render_cloud_objects(&objects, 2, expanded_groups, conn, &mut items);
                    }
                    Err(err_msg) => {
                        let short_err = if err_msg.contains("403") || err_msg.contains("SignatureDoesNotMatch") || err_msg.contains("InvalidAccessKeyId") {
                            "⚠️ Auth failed: Check Key/Secret (press 'e' to edit)"
                        } else if err_msg.contains("404") || err_msg.contains("NoSuchBucket") {
                            "⚠️ Bucket not found (press 'e' to edit URI)"
                        } else if err_msg.contains("Malformed S3 list") {
                            "⚠️ Malformed S3 response (URL_STYLE path required)"
                        } else if err_msg.contains("Connection refused") || err_msg.contains("Could not resolve host") {
                            "⚠️ Connection refused (check endpoint/container)"
                        } else {
                            "⚠️ Inaccessible (press Enter to inspect error)"
                        };
                        items.push(ExplorerNode {
                            name: format!("{}:error:{}", bucket, err_msg),
                            display: format!("    {}", short_err),
                            level: 2,
                            is_expandable: false,
                            is_expanded: false,
                            node_type: NodeType::Group,
                        });
                    }
                }
            }
        }
    }

    items
}

/// Recursively renders cloud files and directory nodes with expandable folder support.
fn render_cloud_objects(
    objects: &[(String, bool)],
    level: usize,
    expanded_groups: &HashSet<String>,
    conn: &duckdb::Connection,
    items: &mut Vec<ExplorerNode>,
) {
    let indent = "  ".repeat(level);
    for (obj_path, is_dir) in objects {
        let filename = obj_path.split('/').last().unwrap_or(obj_path);
        let icon = if *is_dir { "📁" } else if filename.ends_with(".parquet") { "📊" } else if filename.ends_with(".csv") { "📝" } else { "📄" };
        let dir_key = format!("bucket_dir:{}", obj_path);
        let is_expanded = *is_dir && expanded_groups.contains(&dir_key);
        items.push(ExplorerNode {
            name: obj_path.clone(),
            display: format!("{}{} {}", indent, icon, filename),
            level,
            is_expandable: *is_dir,
            is_expanded,
            node_type: NodeType::CloudFile { path: obj_path.clone(), is_dir: *is_dir },
        });

        if is_expanded {
            match crate::catalog::list_s3_bucket_objects(conn, obj_path) {
                Ok(sub_objects) if sub_objects.is_empty() => {
                    items.push(ExplorerNode {
                        name: format!("{}:empty", obj_path),
                        display: format!("{}  (empty folder)", indent),
                        level: level + 1,
                        is_expandable: false,
                        is_expanded: false,
                        node_type: NodeType::Group,
                    });
                }
                Ok(sub_objects) => {
                    render_cloud_objects(&sub_objects, level + 1, expanded_groups, conn, items);
                }
                Err(_) => {}
            }
        }
    }
}

/// Dynamically calculates the required vertical height of the status bar pane
/// based on the length and line wraps of the status message.
pub fn calculate_status_height(status_msg: &str, content_width: usize, total_height: u16) -> u16 {
    let status_inner_width = content_width.saturating_sub(4).max(20);
    let status_line_count: usize = status_msg
        .split('\n')
        .map(|line| {
            let chars = line.chars().count();
            if chars == 0 {
                1
            } else {
                (chars + status_inner_width - 1) / status_inner_width
            }
        })
        .sum();
    let max_status_height = (total_height / 3).clamp(3, 8);
    ((status_line_count as u16) + 2).clamp(3, max_status_height)
}

/// Sets the clipboard text across multiple channels for maximum compatibility:
/// 1. `wl-copy` on Wayland sessions
/// 2. `xclip` on X11 sessions
/// 3. `arboard` (Wayland data-control & X11)
/// 4. OSC 52 ANSI escape sequence for terminal clipboard sync
pub fn set_clipboard_text(text: &str, os_clipboard: &mut Option<arboard::Clipboard>) -> bool {
    let mut success = false;

    // 1. Wayland CLI (wl-copy)
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let res = if text.len() < 65536 {
            std::process::Command::new("wl-copy").arg("--").arg(text).status()
        } else {
            std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    if let Some(mut stdin) = child.stdin.take() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                        drop(stdin);
                    }
                    child.wait()
                })
        };
        if let Ok(status) = res {
            if status.success() {
                success = true;
            }
        }
    }

    // 2. X11 CLI (xclip) if not Wayland or as secondary
    if !success && std::env::var_os("DISPLAY").is_some() {
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            if let Ok(status) = child.wait() {
                if status.success() {
                    success = true;
                }
            }
        }
    }

    // 3. Native arboard
    if os_clipboard.is_none() {
        *os_clipboard = arboard::Clipboard::new().ok();
    }
    if let Some(cb) = os_clipboard {
        if cb.set_text(text).is_ok() {
            success = true;
        }
    }

    // 4. Terminal OSC 52 sequence (universal across SSH, Wayland, X11, macOS, etc.)
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x1b]52;c;{}\x07", encoded);
    let _ = std::io::stdout().flush();

    success || true
}

/// Reads clipboard text from system clipboard, checking wl-paste, arboard, and xclip.
pub fn get_clipboard_text(os_clipboard: &mut Option<arboard::Clipboard>) -> Option<String> {
    // 1. Try Wayland CLI (wl-paste) first if Wayland session
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(output) = std::process::Command::new("wl-paste")
            .args(["--no-newline"])
            .output()
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }

    // 2. Try native arboard
    if os_clipboard.is_none() {
        *os_clipboard = arboard::Clipboard::new().ok();
    }
    if let Some(cb) = os_clipboard {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    // 3. Try X11 CLI (xclip)
    if let Ok(output) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
    {
        if output.status.success() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }

    None
}

/// The main execution loop for the IDE/Explorer feature.
/// Handles rendering the UI layout, capturing keyboard/mouse events, managing tabs,
/// and securely executing raw DuckDB queries from the active text buffer.
pub fn run_ide_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    conn: &Connection,
    project_dir: &PathBuf,
    db_path: &str,
    quack_disable_ssl: bool,
) -> Result<()> {
    let is_quack = db_path.starts_with("quack:");
    let is_remote_db = is_quack || db_path.starts_with("md:") || db_path.starts_with("motherduck:");

    let local_duckdb_version: String = conn
        .query_row("SELECT version();", [], |r| r.get(0))
        .unwrap_or_else(|_| "unknown".to_string());

    let remote_duckdb_version: Option<String> = if is_quack {
        conn.query_row("SELECT * FROM target_db.query('SELECT version()');", [], |r| r.get(0))
            .ok()
    } else if is_remote_db {
        conn.query_row("SELECT version();", [], |r| r.get(0)).ok()
    } else {
        None
    };

    // Eagerly ensure fundamental cloud and analytical extensions (httpfs, parquet) are loaded
    let _ = crate::engine::ensure_extension_loaded(conn, "httpfs");
    let _ = crate::engine::ensure_extension_loaded(conn, "parquet");

    let mut meta = load_explorer_metadata(conn, project_dir, is_quack);
    let mut tabs: Vec<EditorTab> = vec![EditorTab::new(meta.active_database.clone())];
    let mut active_tab_index: usize = 0;
    let mut db_picker_modal = DatabasePickerModalState::new();
    let mut exit_confirm_modal = ExitConfirmationModalState::new();

    let mut focus = FocusPane::Editor;
    let mut status_msg = String::from("Ready. Press 'Tab' to switch panes. 'Esc' to exit.");

    let mut expanded_groups: HashSet<String> = if meta.databases.len() > 1 {
        let mut set = HashSet::new();
        for db in &meta.databases {
            set.insert(format!("db:{}", db.name));
            set.insert(format!("{}:Tables", db.name));
        }
        set.insert("Macros".to_string());
        set.insert("Queries".to_string());
        set
    } else {
        vec!["Tables", "Views", "Macros", "Queries", "Secrets & Cloud Buckets"]
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    };
    let mut expanded_tables: HashSet<String> = HashSet::new();
    let mut attach_modal = AttachModalState::new();
    let mut secret_modal = SecretModalState::new();
    let mut add_bucket_modal = AddBucketModalState::new();

    let mut dictionary = meta.dictionary.clone();
    let mut autocomplete_matches: Vec<String> = Vec::new();
    let mut autocomplete_index = 0;
    let mut autocomplete_original_word = String::new();

    let queries_dir = project_dir.join("queries");

    let mut explorer_mode = ExplorerMode::Database;
    let (mut git_branch, mut git_status) = get_git_branch_and_status(project_dir);
    let mut expanded_project_groups: HashSet<String> = vec![
        "proj:schemas", "proj:tables", "proj:views", "proj:macros", "proj:triggers", "proj:queries", "proj:seeds"
    ].into_iter().map(|s| s.to_string()).collect();

    let mut new_asset_modal = NewAssetModalState::new();
    let mut in_ide_diff_modal = InIdeDiffModalState::new();

    let mut explorer_state = ListState::default();
    let mut explorer_horizontal_scroll: usize = 0;
    let mut explorer_width_percent: u16 = 25;
    let mut explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);

    if !explorer_items.is_empty() {
        explorer_state.select(Some(0));
    }

    let mut current_editor_rect = Rect::default();
    let mut current_explorer_rect = Rect::default();
    let mut current_tabs_rect = Rect::default();
    let mut current_results_rect = Rect::default();
    let mut current_status_rect = Rect::default();
    let mut current_exit_dialog_rect = Rect::default();
    let mut show_help = false;
    let mut show_schema_visualizer = false;
    let mut schema_ascii_tree = String::new();
    let mut schema_scroll = 0;
    let mut max_schema_scroll = 0;
    let mut os_clipboard: Option<arboard::Clipboard> = arboard::Clipboard::new().ok();

    loop {
        let current_target = &tabs[active_tab_index].target_database;
        if !current_target.is_empty() && meta.active_database != *current_target {
            let _ = conn.execute(&format!("USE \"{}\";", current_target.replace('"', "\"\"")), []);
            meta.active_database = current_target.clone();
            for db in &mut meta.databases {
                db.is_active = db.name == *current_target;
            }
        }

        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
                .split(f.area());

            let ssh_prefix = if std::env::var("SSH_CLIENT").is_ok() { "[SSH Remote] " } else { "" };
            let version_badge = if let Some(ref r_ver) = remote_duckdb_version {
                format!(" (Remote: {} | Local: {})", r_ver, local_duckdb_version)
            } else {
                format!(" ({})", local_duckdb_version)
            };
            let top_header = Paragraph::new("")
                .block(Block::default().borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title_top(Line::from(format!(" Target DB: {}{}{} ", ssh_prefix, db_path, version_badge)).alignment(Alignment::Right).style(Style::default().fg(Color::Cyan)))
                    .title_top(Line::from(format!(" DuckPack v{} ", crate::engine::duckpack_version())).alignment(Alignment::Center).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
                );
            f.render_widget(top_header, main_chunks[0]);

            let exp_pct = explorer_width_percent.clamp(15, 60);
            let right_pct = 100 - exp_pct;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(exp_pct), Constraint::Percentage(right_pct)].as_ref())
                .split(main_chunks[1]);

            let is_error_status = status_msg.contains("Error") || status_msg.contains("⚠️") || status_msg.contains("Failed");
            let status_height = calculate_status_height(&status_msg, chunks[1].width as usize, chunks[1].height);

            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(8), Constraint::Percentage(45), Constraint::Length(status_height)].as_ref())
                .split(chunks[1]);

            let max_item_width = explorer_items.iter().map(|item| {
                let indent = item.level * 2;
                let expand_indicator = if item.is_expandable { 2 } else { 0 };
                indent + expand_indicator + item.display.chars().count()
            }).max().unwrap_or(0);
            
            let explorer_inner_width = chunks[0].width.saturating_sub(2) as usize;
            let max_scroll = max_item_width.saturating_sub(explorer_inner_width);
            if explorer_horizontal_scroll > max_scroll {
                explorer_horizontal_scroll = max_scroll;
            }

            // Explorer Pane
            let mut list_items = Vec::new();
            for item in &explorer_items {
                let indent = "  ".repeat(item.level);
                let expand_indicator = if item.is_expandable {
                    if item.is_expanded { "▼ " } else { "▶ " }
                } else {
                    ""
                };
                let display_text = format!("{}{}{}", indent, expand_indicator, item.display)
                    .chars()
                    .skip(explorer_horizontal_scroll)
                    .collect::<String>();
                list_items.push(ListItem::new(display_text));
            }

            let explorer_border_style = if focus == FocusPane::Explorer {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let explorer_title = match explorer_mode {
                ExplorerMode::Database => {
                    if focus == FocusPane::Explorer {
                        format!(" [F3: DB* | F4: Project] Database ({}%) [W: Width | +/-: Resize | q: Query] ", exp_pct)
                    } else {
                        format!(" [F3: DB* | F4: Project] Database ({}%) ", exp_pct)
                    }
                }
                ExplorerMode::Project => {
                    let git_tag = if let Some(ref b) = git_branch {
                        format!(" ({})", b)
                    } else {
                        String::new()
                    };
                    if focus == FocusPane::Explorer {
                        format!(" [F3: DB | F4: Project*] Project{} ({}%) [a: New | Enter: Open | F6: Diff] ", git_tag, exp_pct)
                    } else {
                        format!(" [F3: DB | F4: Project*] Project{} ({}%) ", git_tag, exp_pct)
                    }
                }
            };

            let explorer_list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title(explorer_title).border_style(explorer_border_style))
                .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(explorer_list, chunks[0], &mut explorer_state);
            current_explorer_rect = chunks[0];

            // Tabs Pane
            let tab_titles: Vec<Line> = tabs.iter().enumerate().map(|(i, tab)| {
                let name = if let Some(ref path) = tab.active_file {
                    path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy().to_string()
                } else {
                    format!("New Query {}", i + 1)
                };
                let dirty = if tab.is_dirty { " *" } else { "" };
                let db_badge = if !tab.target_database.is_empty() {
                    format!(" ({})", tab.target_database)
                } else {
                    String::new()
                };
                let style = if i == active_tab_index {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::from(vec![Span::styled(format!(" {}{}{} ", name, db_badge, dirty), style)])
            }).collect();
            
            let tabs_widget = Tabs::new(tab_titles)
                .block(Block::default().borders(Borders::ALL).title(" Open Tabs "))
                .select(active_tab_index)
                .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED));
            f.render_widget(tabs_widget, right_chunks[0]);
            current_tabs_rect = right_chunks[0];

            // Editor Pane
            let active_tab = &mut tabs[active_tab_index];
            let file_display = if let Some(ref path) = active_tab.active_file {
                path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy().to_string()
            } else {
                format!("New Query {}", active_tab_index + 1)
            };
            let dirty = if active_tab.is_dirty { " *" } else { "" };
            let (icon, db_type_tag) = if let Some(d) = meta.databases.iter().find(|d| d.name == active_tab.target_database) {
                (db_icon(&d.db_type), format!(" ({})", d.db_type))
            } else {
                ("📁", String::new())
            };
            let editor_title = if !active_tab.target_database.is_empty() {
                format!(" Editor [{}{}] ── DB: {} {}{} [F2: Switch DB] ", file_display, dirty, icon, active_tab.target_database, db_type_tag)
            } else {
                format!(" Editor [{}{}] [F2: Switch DB] ", file_display, dirty)
            };

            let sql_keywords = SQL_KEYWORD_PATTERN;
            if active_tab.textarea.is_selecting() {
                active_tab.textarea.set_search_pattern("a^").unwrap_or(());
            } else {
                active_tab.textarea.set_search_pattern(sql_keywords).unwrap_or(());
            }

            let editor_border_style = if focus == FocusPane::Editor {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            active_tab.textarea.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(editor_title)
                    .title_bottom(Line::from(" ➕ New (Ctrl+T)  |  ❌ Close (Ctrl+W)  |  💾 Save (Ctrl+S)  |  ✨ Format (Ctrl+F)  |  🗄️ DB (F2) ").alignment(Alignment::Left))
                    .title_bottom(Line::from(" ▶ Run (F5) ").alignment(Alignment::Right))
                    .border_style(editor_border_style)
            );
            f.render_widget(&active_tab.textarea, right_chunks[1]);
            current_editor_rect = right_chunks[1];

            // Results Pane
            let results_border_style = if focus == FocusPane::Results {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let active_tab = &mut tabs[active_tab_index];

            if active_tab.query_results.is_empty() && active_tab.column_names.is_empty() {
                let results_block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Results ")
                    .title_bottom(Line::from(" [?] Help ").alignment(Alignment::Right))
                    .border_style(results_border_style);
                f.render_widget(Paragraph::new("No results.").block(results_block), right_chunks[2]);
            } else {
                let total_cols = active_tab.column_names.len();
                let total_rows = active_tab.query_results.len();

                // Keep horizontal_scroll within bounds
                if total_cols > 0 && active_tab.horizontal_scroll >= total_cols {
                    active_tab.horizontal_scroll = total_cols.saturating_sub(1);
                }

                // Available width for columns inside the results pane (excluding border padding)
                let avail_width = (right_chunks[2].width.saturating_sub(2) as usize).max(10);

                // Dynamically fit columns starting from horizontal_scroll so Ratatui never squashes them to width 0
                let mut visible_indices = Vec::new();
                let mut widths = Vec::new();
                let mut used_width: usize = 0;

                for col_idx in active_tab.horizontal_scroll..total_cols {
                    let col_name = &active_tab.column_names[col_idx];
                    let mut max_len = col_name.len();
                    // Sample first 50 rows for snappy rendering performance
                    for row in active_tab.query_results.iter().take(50) {
                        if let Some(cell) = row.get(col_idx) {
                            max_len = max_len.max(cell.len());
                        }
                    }
                    let col_w = (max_len + 2).clamp(6, 30);

                    // Always show at least 1 column, then pack as many as fit in available width
                    if visible_indices.is_empty() || used_width + col_w <= avail_width {
                        used_width += col_w;
                        widths.push(Constraint::Length(col_w as u16));
                        visible_indices.push(col_idx);
                    } else {
                        break;
                    }
                }

                let (start_col, end_col) = if visible_indices.is_empty() {
                    (0, 0)
                } else {
                    (visible_indices[0] + 1, visible_indices.last().unwrap() + 1)
                };

                let row_info = if active_tab.is_paginated {
                    let offset = active_tab.pagination_offset;
                    format!("Rows {} - {}", offset + 1, offset + total_rows)
                } else {
                    format!("{} rows", total_rows)
                };

                let col_info = if total_cols > 0 {
                    format!(" [Cols {}-{} of {}]", start_col, end_col, total_cols)
                } else {
                    String::new()
                };

                let title = format!(" Results ({}{}) ", row_info, col_info);

                let nav_hint = if total_cols > visible_indices.len() {
                    " ◄/► Scroll Cols (Shift+◄/► fast, Home/End) | [?] Help "
                } else {
                    " [?] Help "
                };

                let results_block = Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_bottom(Line::from(nav_hint).alignment(Alignment::Right))
                    .border_style(results_border_style);

                let header = Row::new(visible_indices.iter().map(|&i| active_tab.column_names[i].as_str()))
                    .style(Style::default().add_modifier(Modifier::BOLD));

                let rows: Vec<Row> = active_tab.query_results.iter().map(|r| {
                    let display_cells: Vec<&str> = visible_indices.iter()
                        .map(|&i| r.get(i).map(|s| s.as_str()).unwrap_or(""))
                        .collect();
                    Row::new(display_cells)
                }).collect();

                let table = Table::new(rows, widths).header(header).block(results_block)
                    .row_highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
                f.render_stateful_widget(table, right_chunks[2], &mut active_tab.results_state);
            }
            current_results_rect = right_chunks[2];

            if exit_confirm_modal.is_open {
                let has_dirty_tabs = tabs.iter().any(|t| t.is_dirty);
                let popup_width = 54.min(f.area().width.saturating_sub(4));
                let popup_height = if has_dirty_tabs { 9 } else { 7 };
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" ⚠️ Exit DuckPack IDE ")
                    .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);
                current_exit_dialog_rect = popup_area;

                let inner = modal_block.inner(popup_area);
                let exit_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(if has_dirty_tabs {
                        vec![
                            Constraint::Length(1), // Question
                            Constraint::Length(1), // Warning
                            Constraint::Min(1),    // Spacer
                            Constraint::Length(1), // Buttons
                        ]
                    } else {
                        vec![
                            Constraint::Length(1), // Question
                            Constraint::Min(1),    // Spacer
                            Constraint::Length(1), // Buttons
                        ]
                    })
                    .split(inner);

                let question = Paragraph::new("Are you sure you want to exit DuckPack?")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
                f.render_widget(question, exit_layout[0]);

                if has_dirty_tabs {
                    let dirty_count = tabs.iter().filter(|t| t.is_dirty).count();
                    let warning_text = if dirty_count == 1 {
                        "⚠️ 1 tab has unsaved changes!".to_string()
                    } else {
                        format!("⚠️ {} tabs have unsaved changes!", dirty_count)
                    };
                    let warning = Paragraph::new(warning_text)
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD));
                    f.render_widget(warning, exit_layout[1]);
                }

                let btn_idx = if has_dirty_tabs { 3 } else { 2 };
                let yes_style = if exit_confirm_modal.confirm_exit {
                    Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let cancel_style = if !exit_confirm_modal.confirm_exit {
                    Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let buttons = Line::from(vec![
                    Span::styled(" [y] Yes, Exit ", yes_style),
                    Span::raw("    "),
                    Span::styled(" [n / Esc] Cancel ", cancel_style),
                ]);
                f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), exit_layout[btn_idx]);
            } else if show_help {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 6,
                    y: f.area().height / 6,
                    width: f.area().width * 2 / 3,
                    height: (f.area().height * 2 / 3).max(18),
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);
                
                let help_lines = vec![
                    Line::from(vec![Span::styled(" [?]       ", Style::default().fg(Color::Cyan)), Span::raw("Toggle Help Menu")]),
                    Line::from(vec![Span::styled(" F2        ", Style::default().fg(Color::Cyan)), Span::raw("Switch Attached Database for Active Tab")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" F3 / 1    ", Style::default().fg(Color::Yellow)), Span::raw("Switch Explorer to Database Catalog")]),
                    Line::from(vec![Span::styled(" F4 / 2    ", Style::default().fg(Color::Yellow)), Span::raw("Switch Explorer to Project Files")]),
                    Line::from(vec![Span::styled(" a / n     ", Style::default().fg(Color::Yellow)), Span::raw("New Project Asset (Table, View, Macro, Schema, Query)")]),
                    Line::from(vec![Span::styled(" Ctrl+B / v", Style::default().fg(Color::Yellow)), Span::raw("Validate Project Syntax & Dependencies (Shadow DB)")]),
                    Line::from(vec![Span::styled(" F6/Ctrl+D ", Style::default().fg(Color::Yellow)), Span::raw("In-IDE Declarative Diff & Apply to Target DB")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" Ctrl+T    ", Style::default().fg(Color::Cyan)), Span::raw("New Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+W    ", Style::default().fg(Color::Cyan)), Span::raw("Close Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+N/P  ", Style::default().fg(Color::Cyan)), Span::raw("Next/Prev Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+E/F5 ", Style::default().fg(Color::Cyan)), Span::raw("Execute Query (or selected text)")]),
                    Line::from(vec![Span::styled(" Ctrl+G    ", Style::default().fg(Color::Cyan)), Span::raw("Visualize Schema (Graph)")]),
                    Line::from(vec![Span::styled(" Ctrl+A    ", Style::default().fg(Color::Cyan)), Span::raw("Attach Database (DuckDB, SQLite, PG, etc.)")]),
                    Line::from(vec![Span::styled(" Ctrl+K    ", Style::default().fg(Color::Cyan)), Span::raw("Cloud Secrets & S3 Buckets (AWS, R2, MinIO)")]),
                    Line::from(vec![Span::styled(" Ctrl+C/V  ", Style::default().fg(Color::Cyan)), Span::raw("Copy / Paste")]),
                    Line::from(vec![Span::styled(" Ctrl+S    ", Style::default().fg(Color::Cyan)), Span::raw("Save Query / Project File")]),
                    Line::from(vec![Span::styled(" Ctrl+F    ", Style::default().fg(Color::Cyan)), Span::raw("Format Query")]),
                    Line::from(vec![Span::styled(" Ctrl+Space", Style::default().fg(Color::Cyan)), Span::raw("Autocomplete")]),
                    Line::from(vec![Span::styled(" n / p     ", Style::default().fg(Color::Cyan)), Span::raw("Next/Prev Page (in Results pane)")]),
                    Line::from(vec![Span::styled(" r / R     ", Style::default().fg(Color::Cyan)), Span::raw("Refresh Explorer (when in Explorer pane)")]),
                    Line::from(vec![Span::styled(" W / Alt+W ", Style::default().fg(Color::Cyan)), Span::raw("Cycle Explorer Width (25% -> 35% -> 50%)")]),
                    Line::from(vec![Span::styled(" +/- or [/]", Style::default().fg(Color::Cyan)), Span::raw("Resize Explorer Width (in 5% increments)")]),
                    Line::from(vec![Span::styled(" ◄ / ► / h/l", Style::default().fg(Color::Cyan)), Span::raw("Scroll Explorer horizontally (Shift: 10 chars, Home: reset)")]),
                    Line::from(vec![Span::styled(" q / s     ", Style::default().fg(Color::Cyan)), Span::raw("Query Table (in Explorer pane)")]),
                    Line::from(vec![Span::styled(" e         ", Style::default().fg(Color::Cyan)), Span::raw("Edit Secret / Edit Bucket (in Explorer pane)")]),
                    Line::from(vec![Span::styled(" a / n     ", Style::default().fg(Color::Cyan)), Span::raw("Add Asset / Secret / Bucket (in Explorer pane)")]),
                    Line::from(vec![Span::styled(" d / Del   ", Style::default().fg(Color::Cyan)), Span::raw("Detach DB / Drop Secret / Remove Bucket (Explorer)")]),
                    Line::from(vec![Span::styled(" ◄ / ►     ", Style::default().fg(Color::Cyan)), Span::raw("Scroll Columns (in Results pane, Shift+◄/► fast)")]),
                    Line::from(vec![Span::styled(" Home/End  ", Style::default().fg(Color::Cyan)), Span::raw("First/Last Column (in Results pane)")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" -- @remote", Style::default().fg(Color::Yellow)), Span::raw("Add at top of query to execute on remote Quack server (e.g. CREATE SECRET)")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" Mouse     ", Style::default().fg(Color::Cyan)), Span::raw("Click tabs/buttons, scroll panes")]),
                ];

                let help_paragraph = Paragraph::new(help_lines)
                        .block(Block::default()
                        .borders(Borders::ALL)
                        .title(" Help Menu ")
                        .border_style(Style::default().fg(Color::Cyan)))
                        .alignment(Alignment::Left);
                f.render_widget(help_paragraph, popup_area);
            } else if show_schema_visualizer {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 8,
                    y: f.area().height / 8,
                    width: (f.area().width * 3) / 4,
                    height: (f.area().height * 3) / 4,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);
                
                let lines: Vec<Line> = schema_ascii_tree.lines().map(|l| Line::from(l.to_string())).collect();
                max_schema_scroll = lines.len().saturating_sub((popup_area.height as usize).saturating_sub(2)) as u16;

                let tree_paragraph = Paragraph::new(lines)
                        .block(Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Schema Visualizer (Scroll: {}/{}) ", schema_scroll, max_schema_scroll))
                        .border_style(Style::default().fg(Color::Cyan)))
                        .alignment(Alignment::Left)
                        .scroll((schema_scroll, 0));
                f.render_widget(tree_paragraph, popup_area);
            } else if db_picker_modal.is_open {
                let popup_width = 58.min(f.area().width.saturating_sub(4));
                let item_count = meta.databases.len().max(1);
                let popup_height = (item_count + 6).min(f.area().height.saturating_sub(2) as usize) as u16;
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" 🗄️ Switch Attached Database (F2) ")
                    .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner = modal_block.inner(popup_area);
                let picker_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1), // Subtitle / hint
                        Constraint::Min(1),    // List of databases
                        Constraint::Length(1), // Controls hint
                    ])
                    .split(inner);

                let sub_hint = Paragraph::new(" Choose target database for current editor tab:")
                    .style(Style::default().fg(Color::DarkGray));
                f.render_widget(sub_hint, picker_layout[0]);

                let list_items: Vec<ListItem> = meta.databases.iter().enumerate().map(|(idx, db)| {
                    let is_selected = idx == db_picker_modal.selected_index;
                    let is_current = db.name == tabs[active_tab_index].target_database;
                    let icon = db_icon(&db.db_type);
                    let current_marker = if is_current { " [Active for Tab]" } else { "" };
                    let type_tag = format!("({})", db.db_type);
                    let cursor = if is_selected { "▶ " } else { "  " };

                    let content = format!("{}{} {} {} {}", cursor, icon, db.name, type_tag, current_marker);
                    let style = if is_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else if is_current {
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(content).style(style)
                }).collect();

                let list_widget = List::new(list_items);
                f.render_widget(list_widget, picker_layout[1]);

                let footer_hint = Paragraph::new(" ▲/▼ or j/k: Navigate | Enter: Select | Esc: Cancel")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray));
                f.render_widget(footer_hint, picker_layout[2]);
            } else if attach_modal.is_open {
                let popup_width = 76.min(f.area().width.saturating_sub(4));
                let err_lines = if attach_modal.error_msg.is_some() { 4 } else { 2 };
                let is_relational = attach_modal.provider().is_relational();
                let popup_height = if is_relational {
                    (19 + err_lines).min(f.area().height.saturating_sub(2))
                } else {
                    (16 + err_lines).min(f.area().height.saturating_sub(2))
                };
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" 🔌 Attach Database (Ctrl+A) ")
                    .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner_area = modal_block.inner(popup_area);

                if is_relational {
                    let field_layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3), // 0: Provider
                            Constraint::Length(3), // 1: Host & Port
                            Constraint::Length(3), // 2: Database & Username
                            Constraint::Length(3), // 3: Password & Alias
                            Constraint::Length(2), // 4: Toggles
                            Constraint::Length(err_lines), // 5: Error/hint
                            Constraint::Length(3), // 6: Buttons
                        ])
                        .split(inner_area);

                    // Field 0: Provider selector
                    let f0_style = if attach_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", attach_modal.provider().name());
                    let p0 = Paragraph::new(prov_label)
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Database Provider Preset ").border_style(f0_style));
                    f.render_widget(p0, field_layout[0]);

                    // Field 1 & 2: Host & Port
                    let host_port_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                        .split(field_layout[1]);
                    let f1_style = if attach_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p1 = Paragraph::new(format!("{}{}", attach_modal.host, if attach_modal.active_field == 1 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Host / Server Address ").border_style(f1_style));
                    f.render_widget(p1, host_port_cols[0]);

                    let f2_style = if attach_modal.active_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p2 = Paragraph::new(format!("{}{}", attach_modal.port, if attach_modal.active_field == 2 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Port ").border_style(f2_style));
                    f.render_widget(p2, host_port_cols[1]);

                    // Field 3 & 4: Database & Username
                    let db_user_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[2]);
                    let f3_style = if attach_modal.active_field == 3 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p3 = Paragraph::new(format!("{}{}", attach_modal.database, if attach_modal.active_field == 3 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Database Name ").border_style(f3_style));
                    f.render_widget(p3, db_user_cols[0]);

                    let user_title = match attach_modal.provider() {
                        AttachProvider::MSSQL => " User ID (e.g. sa) ",
                        AttachProvider::PostgreSQL => " Username (e.g. postgres) ",
                        _ => " Username (e.g. root) ",
                    };
                    let f4_style = if attach_modal.active_field == 4 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p4 = Paragraph::new(format!("{}{}", attach_modal.username, if attach_modal.active_field == 4 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(user_title).border_style(f4_style));
                    f.render_widget(p4, db_user_cols[1]);

                    // Field 5 & 6: Password & Catalog Alias
                    let pass_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[3]);
                    let f5_style = if attach_modal.active_field == 5 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let masked_pass = "*".repeat(attach_modal.password.len());
                    let p5 = Paragraph::new(format!("{}{}", masked_pass, if attach_modal.active_field == 5 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Password (Masked) ").border_style(f5_style));
                    f.render_widget(p5, pass_alias_cols[0]);

                    let f6_style = if attach_modal.active_field == 6 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p6 = Paragraph::new(format!("{}{}", attach_modal.alias, if attach_modal.active_field == 6 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Catalog Alias ").border_style(f6_style));
                    f.render_widget(p6, pass_alias_cols[1]);

                    // Field 7, 8, 9: Toggles
                    let toggle_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
                        .split(field_layout[4]);
                    let f7_style = if attach_modal.active_field == 7 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Read-Only", if attach_modal.read_only { "X" } else { " " })).style(f7_style), toggle_cols[0]);

                    let f8_style = if attach_modal.active_field == 8 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Disable SSL", if attach_modal.disable_ssl { "X" } else { " " })).style(f8_style), toggle_cols[1]);

                    let f9_style = if attach_modal.active_field == 9 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Insecure / Trust", if attach_modal.insecure { "X" } else { " " })).style(f9_style), toggle_cols[2]);

                    // Hint / Error
                    if let Some(ref err) = attach_modal.error_msg {
                        f.render_widget(
                            Paragraph::new(format!(" ⚠️ {}", err))
                                .style(Style::default().fg(Color::LightRed))
                                .wrap(Wrap { trim: true }),
                            field_layout[5],
                        );
                    } else {
                        f.render_widget(
                            Paragraph::new("  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel")
                                .style(Style::default().fg(Color::DarkGray))
                                .wrap(Wrap { trim: true }),
                            field_layout[5],
                        );
                    }

                    // Buttons
                    let attach_btn_style = if attach_modal.active_field == 10 {
                        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let cancel_btn_style = if attach_modal.active_field == 11 {
                        Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let buttons = Line::from(vec![
                        Span::styled(" [ Attach Database ] ", attach_btn_style),
                        Span::raw("    "),
                        Span::styled(" [ Cancel ] ", cancel_btn_style),
                    ]);
                    f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[6]);
                } else if attach_modal.provider().is_quack() {
                    let field_layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3), // 0: Provider
                            Constraint::Length(3), // 1: Host & Port
                            Constraint::Length(3), // 2: Token & Alias
                            Constraint::Length(2), // 3: Toggles
                            Constraint::Length(err_lines), // 4: Error/hint
                            Constraint::Length(3), // 5: Buttons
                        ])
                        .split(inner_area);

                    // Field 0: Provider selector
                    let f0_style = if attach_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", attach_modal.provider().name());
                    let p0 = Paragraph::new(prov_label)
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Database Provider Preset ").border_style(f0_style));
                    f.render_widget(p0, field_layout[0]);

                    // Field 1 & 2: Host & Port
                    let host_port_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                        .split(field_layout[1]);
                    let f1_style = if attach_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p1 = Paragraph::new(format!("{}{}", attach_modal.host, if attach_modal.active_field == 1 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Remote Quack Host / IP (e.g. localhost) ").border_style(f1_style));
                    f.render_widget(p1, host_port_cols[0]);

                    let f2_style = if attach_modal.active_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p2 = Paragraph::new(format!("{}{}", attach_modal.port, if attach_modal.active_field == 2 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Port ").border_style(f2_style));
                    f.render_widget(p2, host_port_cols[1]);

                    // Field 3 & 4: Token & Alias
                    let tok_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[2]);
                    let f3_style = if attach_modal.active_field == 3 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let masked_token = "*".repeat(attach_modal.password.len());
                    let p3 = Paragraph::new(format!("{}{}", masked_token, if attach_modal.active_field == 3 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Auth Token (Optional, Masked) ").border_style(f3_style));
                    f.render_widget(p3, tok_alias_cols[0]);

                    let f4_style = if attach_modal.active_field == 4 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p4 = Paragraph::new(format!("{}{}", attach_modal.alias, if attach_modal.active_field == 4 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Catalog Alias ").border_style(f4_style));
                    f.render_widget(p4, tok_alias_cols[1]);

                    // Field 5, 6, 7: Toggles
                    let toggle_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
                        .split(field_layout[3]);
                    let f5_style = if attach_modal.active_field == 5 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Read-Only", if attach_modal.read_only { "X" } else { " " })).style(f5_style), toggle_cols[0]);

                    let f6_style = if attach_modal.active_field == 6 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Disable SSL", if attach_modal.disable_ssl { "X" } else { " " })).style(f6_style), toggle_cols[1]);

                    let f7_style = if attach_modal.active_field == 7 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Insecure / Trust", if attach_modal.insecure { "X" } else { " " })).style(f7_style), toggle_cols[2]);

                    // Hint / Error
                    if let Some(ref err) = attach_modal.error_msg {
                        f.render_widget(
                            Paragraph::new(format!(" ⚠️ {}", err))
                                .style(Style::default().fg(Color::LightRed))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    } else {
                        f.render_widget(
                            Paragraph::new("  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel")
                                .style(Style::default().fg(Color::DarkGray))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    }

                    // Buttons
                    let attach_btn_style = if attach_modal.active_field == 8 {
                        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let cancel_btn_style = if attach_modal.active_field == 9 {
                        Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let buttons = Line::from(vec![
                        Span::styled(" [ Attach Database ] ", attach_btn_style),
                        Span::raw("    "),
                        Span::styled(" [ Cancel ] ", cancel_btn_style),
                    ]);
                    f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[5]);
                } else if attach_modal.provider().is_motherduck() {
                    let field_layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3), // 0: Provider
                            Constraint::Length(3), // 1: Database Name
                            Constraint::Length(3), // 2: Token & Alias
                            Constraint::Length(2), // 3: Toggles
                            Constraint::Length(err_lines), // 4: Error/hint
                            Constraint::Length(3), // 5: Buttons
                        ])
                        .split(inner_area);

                    // Field 0: Provider selector
                    let f0_style = if attach_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", attach_modal.provider().name());
                    let p0 = Paragraph::new(prov_label)
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Database Provider Preset ").border_style(f0_style));
                    f.render_widget(p0, field_layout[0]);

                    // Field 1: Database Name
                    let f1_style = if attach_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p1 = Paragraph::new(format!("{}{}", attach_modal.path, if attach_modal.active_field == 1 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" MotherDuck Database Name (e.g. my_db or md:my_db) ").border_style(f1_style));
                    f.render_widget(p1, field_layout[1]);

                    // Field 2 & 3: Token & Alias
                    let tok_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[2]);
                    let f2_style = if attach_modal.active_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let masked_token = "*".repeat(attach_modal.password.len());
                    let p2 = Paragraph::new(format!("{}{}", masked_token, if attach_modal.active_field == 2 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Service Token (Optional, Masked) ").border_style(f2_style));
                    f.render_widget(p2, tok_alias_cols[0]);

                    let f3_style = if attach_modal.active_field == 3 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p3 = Paragraph::new(format!("{}{}", attach_modal.alias, if attach_modal.active_field == 3 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Catalog Alias ").border_style(f3_style));
                    f.render_widget(p3, tok_alias_cols[1]);

                    // Field 4: Read-Only Toggle
                    let f4_style = if attach_modal.active_field == 4 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Read-Only", if attach_modal.read_only { "X" } else { " " })).style(f4_style), field_layout[3]);

                    // Hint / Error
                    if let Some(ref err) = attach_modal.error_msg {
                        f.render_widget(
                            Paragraph::new(format!(" ⚠️ {}", err))
                                .style(Style::default().fg(Color::LightRed))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    } else {
                        f.render_widget(
                            Paragraph::new("  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel")
                                .style(Style::default().fg(Color::DarkGray))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    }

                    // Buttons
                    let attach_btn_style = if attach_modal.active_field == 5 {
                        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let cancel_btn_style = if attach_modal.active_field == 6 {
                        Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let buttons = Line::from(vec![
                        Span::styled(" [ Attach Database ] ", attach_btn_style),
                        Span::raw("    "),
                        Span::styled(" [ Cancel ] ", cancel_btn_style),
                    ]);
                    f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[5]);
                } else {
                    let field_layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3), // 0: Provider
                            Constraint::Length(3), // 1: File Path
                            Constraint::Length(3), // 2: Alias
                            Constraint::Length(2), // 3: Read-Only toggle
                            Constraint::Length(err_lines), // 4: Error/hint
                            Constraint::Length(3), // 5: Buttons
                        ])
                        .split(inner_area);

                    // Field 0: Provider selector
                    let f0_style = if attach_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", attach_modal.provider().name());
                    let p0 = Paragraph::new(prov_label)
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Database Provider Preset ").border_style(f0_style));
                    f.render_widget(p0, field_layout[0]);

                    // Field 1: File Path
                    let path_title = if attach_modal.provider() == AttachProvider::DuckDB {
                        " Database File Path (e.g. data.duckdb or :memory:) "
                    } else {
                        " SQLite Database File Path (.db, .sqlite) "
                    };
                    let f1_style = if attach_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p1 = Paragraph::new(format!("{}{}", attach_modal.path, if attach_modal.active_field == 1 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(path_title).border_style(f1_style));
                    f.render_widget(p1, field_layout[1]);

                    // Field 2: Catalog Alias
                    let f2_style = if attach_modal.active_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                    let p2 = Paragraph::new(format!("{}{}", attach_modal.alias, if attach_modal.active_field == 2 { "█" } else { "" }))
                        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Catalog Alias ").border_style(f2_style));
                    f.render_widget(p2, field_layout[2]);

                    // Field 3: Read-Only Toggle
                    let f3_style = if attach_modal.active_field == 3 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                    f.render_widget(Paragraph::new(format!("  [{}] Read-Only", if attach_modal.read_only { "X" } else { " " })).style(f3_style), field_layout[3]);

                    // Hint / Error
                    if let Some(ref err) = attach_modal.error_msg {
                        f.render_widget(
                            Paragraph::new(format!(" ⚠️ {}", err))
                                .style(Style::default().fg(Color::LightRed))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    } else {
                        f.render_widget(
                            Paragraph::new("  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel")
                                .style(Style::default().fg(Color::DarkGray))
                                .wrap(Wrap { trim: true }),
                            field_layout[4],
                        );
                    }

                    // Buttons
                    let attach_btn_style = if attach_modal.active_field == 4 {
                        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    let cancel_btn_style = if attach_modal.active_field == 5 {
                        Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let buttons = Line::from(vec![
                        Span::styled(" [ Attach Database ] ", attach_btn_style),
                        Span::raw("    "),
                        Span::styled(" [ Cancel ] ", cancel_btn_style),
                    ]);
                    f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[5]);
                }
            } else if secret_modal.is_open {
                let popup_width = 72.min(f.area().width.saturating_sub(4));
                let popup_height = 24.min(f.area().height.saturating_sub(2));
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" 🔐 Cloud Secrets & S3 Buckets (Ctrl+K) ")
                    .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner_area = modal_block.inner(popup_area);
                let field_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // 0: Provider
                        Constraint::Length(3), // 1: Name
                        Constraint::Length(3), // 2: Key ID
                        Constraint::Length(3), // 3: Secret Key
                        Constraint::Length(3), // 4: Region & Endpoint
                        Constraint::Length(3), // 5: Bucket
                        Constraint::Length(2), // 6: SSL & Save-to-.env toggles
                        Constraint::Length(1), // 7: Hint / Error
                        Constraint::Length(3), // 8: Buttons
                    ])
                    .split(inner_area);

                // Field 0: Cloud Provider
                let f0_style = if secret_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", secret_modal.provider().name());
                let p0 = Paragraph::new(prov_label)
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Storage Provider ").border_style(f0_style));
                f.render_widget(p0, field_layout[0]);

                // Field 1: Secret Name
                let f1_style = if secret_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p1 = Paragraph::new(format!("{}{}", secret_modal.name, if secret_modal.active_field == 1 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" DuckDB Secret Identifier ").border_style(f1_style));
                f.render_widget(p1, field_layout[1]);

                // Field 2: Key ID
                let f2_style = if secret_modal.active_field == 2 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p2 = Paragraph::new(format!("{}{}", secret_modal.key_id, if secret_modal.active_field == 2 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Access Key ID (AWS_ACCESS_KEY_ID) ").border_style(f2_style));
                f.render_widget(p2, field_layout[2]);

                // Field 3: Secret Key (Masked)
                let f3_style = if secret_modal.active_field == 3 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let masked_secret = "*".repeat(secret_modal.secret_key.len());
                let p3 = Paragraph::new(format!("{}{}", masked_secret, if secret_modal.active_field == 3 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Secret Access Key (AWS_SECRET_ACCESS_KEY, Masked) ").border_style(f3_style));
                f.render_widget(p3, field_layout[3]);

                // Field 4: Region & Endpoint
                let reg_end_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(field_layout[4]);

                let f4_reg_style = if secret_modal.active_field == 4 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p_reg = Paragraph::new(format!("{}{}", secret_modal.region, if secret_modal.active_field == 4 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Region ").border_style(f4_reg_style));
                f.render_widget(p_reg, reg_end_cols[0]);

                let f5_end_style = if secret_modal.active_field == 5 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p_end = Paragraph::new(format!("{}{}", secret_modal.endpoint, if secret_modal.active_field == 5 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Custom Endpoint (Optional) ").border_style(f5_end_style));
                f.render_widget(p_end, reg_end_cols[1]);

                // Field 5: Bucket
                let f6_style = if secret_modal.active_field == 6 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p6 = Paragraph::new(format!("{}{}", secret_modal.bucket, if secret_modal.active_field == 6 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Default Bucket to Explore (e.g. s3://my-bucket) ").border_style(f6_style));
                f.render_widget(p6, field_layout[5]);

                // Field 6: SSL & Save-to-.env toggles
                let toggle_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(field_layout[6]);

                let f7_ssl_style = if secret_modal.active_field == 7 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                let ssl_label = format!("  [{}] Use SSL / HTTPS (Space)", if secret_modal.use_ssl { "X" } else { " " });
                f.render_widget(Paragraph::new(ssl_label).style(f7_ssl_style), toggle_cols[0]);

                let f8_env_style = if secret_modal.active_field == 8 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                let env_label = format!("  [{}] Persist in .env (Space)", if secret_modal.save_to_env { "X" } else { " " });
                f.render_widget(Paragraph::new(env_label).style(f8_env_style), toggle_cols[1]);

                // Field 7: Error or navigation hint
                if let Some(ref err) = secret_modal.error_msg {
                    f.render_widget(
                        Paragraph::new(format!(" ⚠️ {}", err))
                            .style(Style::default().fg(Color::LightRed))
                            .wrap(Wrap { trim: true }),
                        field_layout[7],
                    );
                } else {
                    f.render_widget(
                        Paragraph::new("  Tab: Next Field | Space: Toggle | Enter: Save Secret | Esc: Cancel")
                            .style(Style::default().fg(Color::DarkGray))
                            .wrap(Wrap { trim: true }),
                        field_layout[7],
                    );
                }

                // Field 8: Buttons
                let save_btn_style = if secret_modal.active_field == 9 {
                    Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let cancel_btn_style = if secret_modal.active_field == 10 {
                    Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let buttons = Line::from(vec![
                    Span::styled(" [ Save & Attach Secret ] ", save_btn_style),
                    Span::raw("    "),
                    Span::styled(" [ Cancel ] ", cancel_btn_style),
                ]);
                f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[8]);
            } else if add_bucket_modal.is_open {
                let popup_width = 64.min(f.area().width.saturating_sub(4));
                let popup_height = 14.min(f.area().height.saturating_sub(2));
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" 🪣 Add S3 / Cloud Bucket ")
                    .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner_area = modal_block.inner(popup_area);
                let field_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // 0: Bucket URI
                        Constraint::Length(2), // 1: Persist in .env toggle
                        Constraint::Length(2), // 2: Hint / error
                        Constraint::Length(3), // 3: Buttons
                    ])
                    .split(inner_area);

                // Field 0: URI
                let f0_style = if add_bucket_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p0 = Paragraph::new(format!("{}{}", add_bucket_modal.uri, if add_bucket_modal.active_field == 0 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Bucket URI or Name (e.g. s3://my-lake) ").border_style(f0_style));
                f.render_widget(p0, field_layout[0]);

                // Field 1: Persist toggle
                let f1_style = if add_bucket_modal.active_field == 1 { Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::White) };
                let env_label = format!("  [{}] Persist to .env (Space to toggle)", if add_bucket_modal.save_to_env { "X" } else { " " });
                f.render_widget(Paragraph::new(env_label).style(f1_style), field_layout[1]);

                // Field 2: Hint / Error
                if let Some(ref err) = add_bucket_modal.error_msg {
                    f.render_widget(
                        Paragraph::new(format!(" ⚠️ {}", err))
                            .style(Style::default().fg(Color::LightRed))
                            .wrap(Wrap { trim: true }),
                        field_layout[2],
                    );
                } else {
                    f.render_widget(
                        Paragraph::new("  Tab: Next Field | Space: Toggle | Enter: Add Bucket | Esc: Cancel")
                            .style(Style::default().fg(Color::DarkGray))
                            .wrap(Wrap { trim: true }),
                        field_layout[2],
                    );
                }

                // Field 3: Buttons
                let add_btn_style = if add_bucket_modal.active_field == 2 {
                    Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let cancel_btn_style = if add_bucket_modal.active_field == 3 {
                    Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let buttons = Line::from(vec![
                    Span::styled(" [ Add Bucket ] ", add_btn_style),
                    Span::raw("    "),
                    Span::styled(" [ Cancel ] ", cancel_btn_style),
                ]);
                f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[3]);
            } else if new_asset_modal.is_open {
                let popup_width = 62.min(f.area().width.saturating_sub(4));
                let popup_height = 16.min(f.area().height.saturating_sub(2));
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" ✨ New Project Asset (a / n) ")
                    .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner_area = modal_block.inner(popup_area);
                let field_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // 0: Category
                        Constraint::Length(3), // 1: Name
                        Constraint::Length(2), // 2: Filepath preview
                        Constraint::Length(2), // 3: Hint/Error
                        Constraint::Length(3), // 4: Buttons
                    ])
                    .split(inner_area);

                // Field 0: Category
                let f0_style = if new_asset_modal.active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let cat_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", new_asset_modal.category_label());
                let p0 = Paragraph::new(cat_label)
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Asset Type ").border_style(f0_style));
                f.render_widget(p0, field_layout[0]);

                // Field 1: Name
                let f1_style = if new_asset_modal.active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) };
                let p1 = Paragraph::new(format!("{}{}", new_asset_modal.name, if new_asset_modal.active_field == 1 { "█" } else { "" }))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Asset Name (e.g. users, sales_summary) ").border_style(f1_style));
                f.render_widget(p1, field_layout[1]);

                // Field 2: Preview target path
                let target_path = if new_asset_modal.name.trim().is_empty() {
                    "<enter name>".to_string()
                } else {
                    let p = get_asset_file_path(project_dir, new_asset_modal.current_category(), &new_asset_modal.name);
                    p.strip_prefix(project_dir).unwrap_or(&p).display().to_string()
                };
                let p_preview = Paragraph::new(format!("  📁 Target File: {}", target_path)).style(Style::default().fg(Color::Cyan));
                f.render_widget(p_preview, field_layout[2]);

                // Field 3: Error or Hint
                if let Some(ref err) = new_asset_modal.error_msg {
                    f.render_widget(
                        Paragraph::new(format!(" ⚠️ {}", err))
                            .style(Style::default().fg(Color::LightRed))
                            .wrap(Wrap { trim: true }),
                        field_layout[3],
                    );
                } else {
                    f.render_widget(
                        Paragraph::new("  Tab: Next Field | Enter: Create & Open | Esc: Cancel")
                            .style(Style::default().fg(Color::DarkGray))
                            .wrap(Wrap { trim: true }),
                        field_layout[3],
                    );
                }

                // Field 4: Buttons
                let create_btn_style = if new_asset_modal.active_field == 2 {
                    Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let cancel_btn_style = if new_asset_modal.active_field == 3 {
                    Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let buttons = Line::from(vec![
                    Span::styled(" [ Create & Open ] ", create_btn_style),
                    Span::raw("    "),
                    Span::styled(" [ Cancel ] ", cancel_btn_style),
                ]);
                f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), field_layout[4]);
            } else if in_ide_diff_modal.is_open {
                let popup_width = 80.min(f.area().width.saturating_sub(4));
                let popup_height = 22.min(f.area().height.saturating_sub(2));
                let popup_area = ratatui::layout::Rect {
                    x: (f.area().width.saturating_sub(popup_width)) / 2,
                    y: (f.area().height.saturating_sub(popup_height)) / 2,
                    width: popup_width,
                    height: popup_height,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let modal_block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" 🔍 Declarative Diff & Apply (F6 / Ctrl+D) ")
                    .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                f.render_widget(modal_block.clone(), popup_area);

                let inner_area = modal_block.inner(popup_area);
                let err_height = if in_ide_diff_modal.error_msg.is_some() { 3 } else { 2 };
                let diff_layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2), // 0: Summary header
                        Constraint::Min(6),    // 1: Changes list
                        Constraint::Length(err_height), // 2: Error message or hint
                        Constraint::Length(3), // 3: Action buttons
                    ])
                    .split(inner_area);

                if let Some(ref diff) = in_ide_diff_modal.diff_result {
                    let active_indices: Vec<usize> = diff.items.iter().enumerate().filter(|(_, i)| !i.ignored).map(|(idx, _)| idx).collect();
                    if active_indices.is_empty() {
                        let msg = Paragraph::new("\n  ✨ Target database catalog is completely up to date with project files!\n\n  No pending migrations or schema changes detected.")
                            .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));
                        f.render_widget(msg, diff_layout[1]);

                        let close_btn = Line::from(Span::styled(" [ Close (Esc) ] ", Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)));
                        f.render_widget(Paragraph::new(close_btn).alignment(Alignment::Center), diff_layout[3]);
                    } else {
                        let selected_count = active_indices.iter().filter(|&&idx| diff.items[idx].selected && !diff.items[idx].locked).count();
                        let summary_text = format!(" Found {} pending declarative change(s) ({} selected):", active_indices.len(), selected_count);
                        f.render_widget(Paragraph::new(summary_text).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)), diff_layout[0]);

                        let list_lines: Vec<Line> = active_indices.iter().enumerate().map(|(pos, &real_idx)| {
                            let item = &diff.items[real_idx];
                            let (badge, badge_style) = match item.item_type {
                                crate::diff::DiffItemType::CreateTable | crate::diff::DiffItemType::CreateSchema | crate::diff::DiffItemType::CreateView | crate::diff::DiffItemType::CreateMacro | crate::diff::DiffItemType::CreateTrigger => {
                                    ("[+]", Style::default().fg(Color::Green))
                                }
                                crate::diff::DiffItemType::AlterTable | crate::diff::DiffItemType::AddConstraint | crate::diff::DiffItemType::RenameTable | crate::diff::DiffItemType::RenameColumn => {
                                    ("[~]", Style::default().fg(Color::Yellow))
                                }
                                crate::diff::DiffItemType::RecreateTable => {
                                    ("[⚡]", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))
                                }
                                crate::diff::DiffItemType::DropTable | crate::diff::DiffItemType::DropSchema | crate::diff::DiffItemType::DropView | crate::diff::DiffItemType::DropMacro | crate::diff::DiffItemType::DropColumn | crate::diff::DiffItemType::DropTrigger => {
                                    ("[-]", Style::default().fg(Color::Red))
                                }
                            };
                            let cursor = if pos == in_ide_diff_modal.selected_index { "▶ " } else { "  " };
                            let (checkbox, item_style) = if item.locked {
                                (
                                    Span::styled("[L] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                                    Style::default().fg(Color::Red).add_modifier(Modifier::CROSSED_OUT),
                                )
                            } else if item.selected {
                                (
                                    Span::styled("[x] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                                    badge_style,
                                )
                            } else {
                                (
                                    Span::styled("[ ] ", Style::default().fg(Color::DarkGray)),
                                    Style::default().fg(Color::DarkGray),
                                )
                            };

                            Line::from(vec![
                                Span::styled(cursor, Style::default().fg(Color::Yellow)),
                                checkbox,
                                Span::styled(format!("{} ", badge), item_style.add_modifier(Modifier::BOLD)),
                                Span::styled(
                                    format!("{:?}: {}", item.item_type, item.obj_name),
                                    if item.locked {
                                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT)
                                    } else if !item.selected {
                                        Style::default().fg(Color::DarkGray)
                                    } else {
                                        Style::default()
                                    },
                                ),
                            ])
                        }).collect();

                        let visible_height = diff_layout[1].height.saturating_sub(2) as usize;
                        let scroll_top = if in_ide_diff_modal.selected_index >= visible_height {
                            (in_ide_diff_modal.selected_index + 1).saturating_sub(visible_height)
                        } else {
                            0
                        };

                        let changes_list = Paragraph::new(list_lines)
                            .scroll((scroll_top as u16, 0))
                            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Pending Changes "));
                        f.render_widget(changes_list, diff_layout[1]);

                        if let Some(ref err) = in_ide_diff_modal.error_msg {
                            f.render_widget(
                                Paragraph::new(format!(" ⚠️ {}", err))
                                    .style(Style::default().fg(Color::LightRed))
                                    .wrap(Wrap { trim: true }),
                                diff_layout[2],
                            );
                        } else {
                            f.render_widget(Paragraph::new("  Space: Toggle Selection | a: Toggle All | Enter: Apply Selected | Esc: Cancel").style(Style::default().fg(Color::DarkGray)), diff_layout[2]);
                        }

                        let apply_btn_style = if selected_count > 0 {
                            Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().bg(Color::DarkGray).fg(Color::Gray)
                        };
                        let close_btn_style = Style::default().fg(Color::DarkGray);
                        let buttons = Line::from(vec![
                            Span::styled(format!(" [ Apply {} Selected Change(s) (Enter) ] ", selected_count), apply_btn_style),
                            Span::raw("    "),
                            Span::styled(" [ Cancel (Esc) ] ", close_btn_style),
                        ]);
                        f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), diff_layout[3]);
                    }
                } else {
                    let loading = Paragraph::new(" Computing diff against target database...").style(Style::default().fg(Color::Cyan));
                    f.render_widget(loading, diff_layout[1]);
                }
            }

            // Status Bar
            let status_title = if status_msg.starts_with("📋 [Copied] ") {
                " Status [Copied to clipboard! 📋] "
            } else if is_error_status {
                " Status / Error [Click box to copy 📋] "
            } else {
                " Status [Click box to copy 📋] "
            };

            let status_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .border_style(if is_error_status {
                    Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                })
                .title(Line::from(status_title).style(if is_error_status {
                    Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                }));

            let status = Paragraph::new(status_msg.as_str())
                .block(status_block)
                .style(if is_error_status {
                    Style::default().fg(Color::LightRed)
                } else {
                    Style::default().fg(Color::White)
                })
                .wrap(Wrap { trim: false });
            f.render_widget(status, right_chunks[3]);
            current_status_rect = right_chunks[3];

        })?;

        let mut execute_query = false;
        let mut save_query = false;
        let mut export_query = false;
        let mut format_query = false;
        let mut new_tab = false;
        let mut close_tab = false;
        let mut trigger_explorer_action = false;
        let mut trigger_explorer_refresh = false;
        let mut trigger_pagination: Option<i32> = None;
        let ev = event::read()?;

        match ev {
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if exit_confirm_modal.is_open {
                        if mouse.row == current_exit_dialog_rect.bottom().saturating_sub(2) {
                            let center_x = current_exit_dialog_rect.left() + current_exit_dialog_rect.width / 2;
                            if mouse.column >= current_exit_dialog_rect.left() && mouse.column < center_x {
                                break;
                            } else if mouse.column >= center_x && mouse.column <= current_exit_dialog_rect.right() {
                                exit_confirm_modal.close();
                                status_msg = "Exit cancelled".to_string();
                            }
                        } else if mouse.row < current_exit_dialog_rect.top() || mouse.row >= current_exit_dialog_rect.bottom()
                            || mouse.column < current_exit_dialog_rect.left() || mouse.column >= current_exit_dialog_rect.right() {
                            exit_confirm_modal.close();
                            status_msg = "Exit cancelled".to_string();
                        }
                    } else if mouse.row >= current_status_rect.top() && mouse.row < current_status_rect.bottom()
                        && mouse.column >= current_status_rect.left() && mouse.column < current_status_rect.right()
                    {
                        let clean_text = if let Some(stripped) = status_msg.strip_prefix("📋 [Copied] ") {
                            stripped.to_string()
                        } else {
                            status_msg.clone()
                        };
                        if !clean_text.is_empty() {
                            if set_clipboard_text(&clean_text, &mut os_clipboard) {
                                if !status_msg.starts_with("📋 [Copied] ") {
                                    status_msg = format!("📋 [Copied] {}", status_msg);
                                }
                            }
                        }
                    } else if mouse.row == current_editor_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_editor_rect.right().saturating_sub(15) && mouse.column <= current_editor_rect.right() {
                            execute_query = true;
                        }
                        if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.left() + 18 {
                            new_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 18 && mouse.column <= current_editor_rect.left() + 40 {
                            close_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 40 && mouse.column <= current_editor_rect.left() + 61 {
                            save_query = true;
                        }
                        if mouse.column > current_editor_rect.left() + 61 && mouse.column <= current_editor_rect.left() + 85 {
                            format_query = true;
                        }
                        if mouse.column > current_editor_rect.left() + 85 && mouse.column <= current_editor_rect.left() + 105 {
                            db_picker_modal.open_for_current(&tabs[active_tab_index].target_database, &meta.databases);
                        }
                    } else if mouse.row == current_results_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_results_rect.right().saturating_sub(10) && mouse.column <= current_results_rect.right() {
                            show_help = !show_help;
                        }
                    } else if mouse.row >= current_editor_rect.top() && mouse.row < current_editor_rect.bottom() {
                        if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            focus = FocusPane::Editor;
                        }
                    }

                    // Explorer Click
                    if mouse.row == current_explorer_rect.top() {
                        if mouse.column >= current_explorer_rect.left() + 1 && mouse.column <= current_explorer_rect.left() + 8 {
                            explorer_mode = ExplorerMode::Database;
                            explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Database Catalog view".to_string();
                        } else if mouse.column >= current_explorer_rect.left() + 9 && mouse.column <= current_explorer_rect.left() + 22 {
                            explorer_mode = ExplorerMode::Project;
                            let (b, st) = get_git_branch_and_status(project_dir);
                            git_branch = b;
                            git_status = st;
                            explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Project Files view".to_string();
                        } else if explorer_mode == ExplorerMode::Project && mouse.column >= current_explorer_rect.right().saturating_sub(20) && mouse.column <= current_explorer_rect.right() {
                            new_asset_modal.reset();
                        } else if explorer_mode == ExplorerMode::Database && mouse.column >= current_explorer_rect.right().saturating_sub(18) && mouse.column <= current_explorer_rect.right() {
                            attach_modal.reset();
                        }
                    } else if mouse.row >= current_explorer_rect.top() + 1 && mouse.row < current_explorer_rect.bottom() - 1 {
                        if mouse.column >= current_explorer_rect.left() + 1 && mouse.column < current_explorer_rect.right() - 1 {
                            let list_offset = explorer_state.offset();
                            let clicked_idx = (mouse.row - current_explorer_rect.top() - 1) as usize + list_offset;
                            if clicked_idx < explorer_items.len() {
                                focus = FocusPane::Explorer;
                                explorer_state.select(Some(clicked_idx));
                                trigger_explorer_action = true;
                            }
                        }
                    }

                    // Tabs Click
                    if mouse.row == current_tabs_rect.top() + 1 && mouse.column >= current_tabs_rect.left() + 1 && mouse.column < current_tabs_rect.right() - 1 {
                        let mut current_x = current_tabs_rect.left() + 1;
                        for (i, tab) in tabs.iter().enumerate() {
                            let tab_name = if let Some(ref path) = tab.active_file {
                                path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy().to_string()
                            } else {
                                format!("New Query {}", i + 1)
                            };
                            let dirty = if tab.is_dirty { " *" } else { "" };
                            let db_badge = if !tab.target_database.is_empty() {
                                format!(" ({})", tab.target_database)
                            } else {
                                String::new()
                            };
                            let tab_width = (tab_name.chars().count() + db_badge.chars().count() + dirty.chars().count() + 2) as u16;
                            let display_width = tab_width + 3; // Ratatui divider " | "
                            if mouse.column >= current_x && mouse.column < current_x + display_width {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                break;
                            }
                            current_x += display_width;
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollDown {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            explorer_horizontal_scroll += 4;
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let max_col = tabs[active_tab_index].column_names.len().saturating_sub(1);
                            if tabs[active_tab_index].horizontal_scroll < max_col {
                                tabs[active_tab_index].horizontal_scroll += 1;
                            }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((0, 1));
                        }
                    } else {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len().saturating_sub(1) {
                                    explorer_state.select(Some(selected + 1));
                                }
                            } else if !explorer_items.is_empty() {
                                explorer_state.select(Some(0));
                            }
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if let Some(selected) = active_tab.results_state.selected() {
                                if selected < active_tab.query_results.len().saturating_sub(1) {
                                    active_tab.results_state.select(Some(selected + 1));
                                }
                            } else if !active_tab.query_results.is_empty() {
                                active_tab.results_state.select(Some(0));
                            }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((1, 0));
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollUp {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            explorer_horizontal_scroll = explorer_horizontal_scroll.saturating_sub(4);
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if active_tab.horizontal_scroll > 0 { active_tab.horizontal_scroll -= 1; }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((0, -1));
                        }
                    } else {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            if let Some(selected) = explorer_state.selected() {
                                if selected > 0 {
                                    explorer_state.select(Some(selected - 1));
                                }
                            }
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if let Some(selected) = active_tab.results_state.selected() {
                                if selected > 0 {
                                    active_tab.results_state.select(Some(selected - 1));
                                }
                            }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((-1, 0));
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollLeft {
                    if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                        explorer_horizontal_scroll = explorer_horizontal_scroll.saturating_sub(4);
                    } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                        let active_tab = &mut tabs[active_tab_index];
                        if active_tab.horizontal_scroll > 0 {
                            active_tab.horizontal_scroll -= 1;
                        }
                    } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        // For textarea, scroll((lines, columns))
                        tabs[active_tab_index].textarea.scroll((0, -1));
                    }
                } else if mouse.kind == MouseEventKind::ScrollRight {
                    if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                        explorer_horizontal_scroll += 4;
                    } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                        let max_col = tabs[active_tab_index].column_names.len().saturating_sub(1);
                        if tabs[active_tab_index].horizontal_scroll < max_col {
                            tabs[active_tab_index].horizontal_scroll += 1;
                        }
                    } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        tabs[active_tab_index].textarea.scroll((0, 1));
                    }
                }

                // Forward mouse events to textarea if inside editor bounds
                if mouse.row >= current_editor_rect.top() && mouse.row < current_editor_rect.bottom() - 1 {
                    if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        if focus == FocusPane::Editor {
                            tabs[active_tab_index].textarea.input(ev.clone());
                        }
                    }
                }
            }
            Event::Key(key) => {
                if exit_confirm_modal.is_open {
                    if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                        break;
                    } else if key.code == KeyCode::Char('n') || key.code == KeyCode::Char('N') || key.code == KeyCode::Esc {
                        exit_confirm_modal.close();
                        status_msg = "Exit cancelled".to_string();
                    } else if key.code == KeyCode::Left || key.code == KeyCode::Right || key.code == KeyCode::Tab || key.code == KeyCode::BackTab || key.code == KeyCode::Char('h') || key.code == KeyCode::Char('l') {
                        exit_confirm_modal.toggle();
                    } else if key.code == KeyCode::Enter {
                        if exit_confirm_modal.confirm_exit {
                            break;
                        } else {
                            exit_confirm_modal.close();
                            status_msg = "Exit cancelled".to_string();
                        }
                    }
                } else if new_asset_modal.is_open {
                    if key.code == KeyCode::Esc {
                        new_asset_modal.is_open = false;
                    } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
                        new_asset_modal.active_field = (new_asset_modal.active_field + 1) % 4;
                    } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
                        new_asset_modal.active_field = if new_asset_modal.active_field == 0 { 3 } else { new_asset_modal.active_field - 1 };
                    } else if new_asset_modal.active_field == 0 && (key.code == KeyCode::Left || key.code == KeyCode::Char('h')) {
                        new_asset_modal.category_index = if new_asset_modal.category_index == 0 { 6 } else { new_asset_modal.category_index - 1 };
                    } else if new_asset_modal.active_field == 0 && (key.code == KeyCode::Right || key.code == KeyCode::Char('l')) {
                        new_asset_modal.category_index = (new_asset_modal.category_index + 1) % 7;
                    } else if new_asset_modal.active_field >= 2 && (key.code == KeyCode::Left || key.code == KeyCode::Right) {
                        new_asset_modal.active_field = if new_asset_modal.active_field == 2 { 3 } else { 2 };
                    } else if key.code == KeyCode::Backspace {
                        if new_asset_modal.active_field == 1 {
                            new_asset_modal.name.pop();
                        }
                    } else if key.code == KeyCode::Enter {
                        if new_asset_modal.active_field == 3 {
                            new_asset_modal.is_open = false;
                        } else {
                            let asset_name = new_asset_modal.name.trim().to_string();
                            if asset_name.is_empty() {
                                new_asset_modal.error_msg = Some("Asset name cannot be empty.".to_string());
                            } else if let Err(e) = crate::diff::validate_safe_project_filename(&asset_name) {
                                new_asset_modal.error_msg = Some(e.to_string());
                            } else {
                                let cat = new_asset_modal.current_category();
                                let path = get_asset_file_path(project_dir, cat, &asset_name);
                                if let Some(parent) = path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                if !path.exists() {
                                    let boilerplate = generate_asset_boilerplate(cat, &asset_name);
                                    let _ = std::fs::write(&path, boilerplate);
                                }
                                
                                let mut already_open = None;
                                for (i, tab) in tabs.iter().enumerate() {
                                    if tab.active_file.as_ref() == Some(&path) {
                                        already_open = Some(i);
                                        break;
                                    }
                                }
                                if let Some(i) = already_open {
                                    active_tab_index = i;
                                } else if let Ok(content) = std::fs::read_to_string(&path) {
                                    let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                    if !current_is_empty {
                                        tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                        active_tab_index = tabs.len() - 1;
                                    }
                                    tabs[active_tab_index].textarea = TextArea::from(content.lines().map(|s| s.to_string()).collect::<Vec<_>>());
                                    tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                    tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                    tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                    tabs[active_tab_index].active_file = Some(path.clone());
                                    tabs[active_tab_index].is_dirty = false;
                                }
                                new_asset_modal.is_open = false;
                                focus = FocusPane::Editor;
                                trigger_explorer_refresh = true;
                                status_msg = format!("Created and opened {}", path.file_name().unwrap_or_default().to_string_lossy());
                            }
                        }
                    } else if let KeyCode::Char(c) = key.code {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && new_asset_modal.active_field == 1 {
                            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                                new_asset_modal.name.push(c);
                            }
                        }
                    }
                } else if in_ide_diff_modal.is_open {
                    if key.code == KeyCode::Esc {
                        in_ide_diff_modal.is_open = false;
                    } else if key.code == KeyCode::Char(' ') {
                        if let Some(ref mut diff) = in_ide_diff_modal.diff_result {
                            let active_indices: Vec<usize> = diff.items.iter().enumerate().filter(|(_, i)| !i.ignored).map(|(idx, _)| idx).collect();
                            if in_ide_diff_modal.selected_index < active_indices.len() {
                                let real_idx = active_indices[in_ide_diff_modal.selected_index];
                                if !diff.items[real_idx].locked && !diff.items[real_idx].ignored {
                                    diff.items[real_idx].selected = !diff.items[real_idx].selected;
                                    crate::tui::recompute_locks(&mut diff.items);
                                    in_ide_diff_modal.error_msg = None;
                                } else if diff.items[real_idx].locked {
                                    in_ide_diff_modal.error_msg = Some(format!(
                                        "Cannot select '{}': dependent on a deselected item.",
                                        diff.items[real_idx].obj_name
                                    ));
                                }
                            }
                        }
                    } else if key.code == KeyCode::Char('a') || key.code == KeyCode::Char('A') {
                        if let Some(ref mut diff) = in_ide_diff_modal.diff_result {
                            let any_unselected = diff.items.iter().any(|i| !i.ignored && !i.locked && !i.selected);
                            for item in diff.items.iter_mut() {
                                if !item.ignored {
                                    item.selected = any_unselected;
                                }
                            }
                            crate::tui::recompute_locks(&mut diff.items);
                            in_ide_diff_modal.error_msg = None;
                        }
                    } else if key.code == KeyCode::Enter {
                        if let Some(ref diff) = in_ide_diff_modal.diff_result {
                            let active_indices: Vec<usize> = diff.items.iter().enumerate().filter(|(_, i)| !i.ignored).map(|(idx, _)| idx).collect();
                            if active_indices.is_empty() {
                                in_ide_diff_modal.is_open = false;
                            } else {
                                let selected_count = active_indices.iter().filter(|&&idx| diff.items[idx].selected && !diff.items[idx].locked).count();
                                if selected_count > 0 {
                                    let tab_cat = tabs.get(active_tab_index).map(|t| t.target_database.as_str()).filter(|c| !c.is_empty() && *c != "memory");
                                    match apply_ide_diff_for_catalog(project_dir, conn, diff, tab_cat) {
                                        Ok(_) => {
                                            status_msg = format!("✓ Applied {} selected change(s) to target catalog successfully!", selected_count);
                                            in_ide_diff_modal.is_open = false;
                                            trigger_explorer_refresh = true;
                                        }
                                        Err(e) => {
                                            in_ide_diff_modal.error_msg = Some(format!("Apply Error: {}", e));
                                        }
                                    }
                                } else {
                                    in_ide_diff_modal.error_msg = Some("No changes selected to apply. Press Space to select changes or Esc to cancel.".to_string());
                                }
                            }
                        } else {
                            in_ide_diff_modal.is_open = false;
                        }
                    } else if key.code == KeyCode::Up {
                        if in_ide_diff_modal.selected_index > 0 {
                            in_ide_diff_modal.selected_index -= 1;
                        }
                    } else if key.code == KeyCode::Down {
                        if let Some(ref diff) = in_ide_diff_modal.diff_result {
                            let total = diff.items.iter().filter(|i| !i.ignored).count();
                            if in_ide_diff_modal.selected_index + 1 < total {
                                in_ide_diff_modal.selected_index += 1;
                            }
                        }
                    }
                } else if attach_modal.is_open {
                    if key.code == KeyCode::Esc {
                        attach_modal.is_open = false;
                    } else if key.code == KeyCode::Tab {
                        attach_modal.next_field();
                    } else if key.code == KeyCode::BackTab {
                        attach_modal.prev_field();
                    } else if key.code == KeyCode::Down {
                        attach_modal.nav_down();
                    } else if key.code == KeyCode::Up {
                        attach_modal.nav_up();
                    } else if key.code == KeyCode::Left || (attach_modal.active_field == 0 && key.code == KeyCode::Char('h')) {
                        attach_modal.nav_left();
                    } else if key.code == KeyCode::Right || (attach_modal.active_field == 0 && key.code == KeyCode::Char('l')) {
                        attach_modal.nav_right();
                    } else if key.code == KeyCode::Char(' ') && attach_modal.is_toggle_field() {
                        attach_modal.toggle_active();
                    } else if key.code == KeyCode::Enter {
                        if attach_modal.is_cancel() {
                            attach_modal.is_open = false;
                        } else if attach_modal.is_toggle_field() {
                            attach_modal.toggle_active();
                        } else {
                            match attach_modal.build_connection_target() {
                                Err(err) => {
                                    attach_modal.error_msg = Some(err);
                                }
                                Ok((safe_path, alias)) => {
                                    let provider = attach_modal.provider();
                                    let ext_res = match provider {
                                        AttachProvider::SQLite => crate::engine::ensure_extension_loaded(conn, "sqlite"),
                                        AttachProvider::PostgreSQL => crate::engine::ensure_extension_loaded(conn, "postgres"),
                                        AttachProvider::MySQL => crate::engine::ensure_extension_loaded(conn, "mysql"),
                                        AttachProvider::MSSQL => crate::engine::ensure_extension_loaded(conn, "mssql"),
                                        AttachProvider::MotherDuck => crate::engine::ensure_extension_loaded(conn, "motherduck"),
                                        AttachProvider::Quack => crate::engine::load_quack_extension(conn),
                                        AttachProvider::DuckDB => Ok(()),
                                    };

                                    if let Err(e) = ext_res {
                                        attach_modal.error_msg = Some(format!("Failed to load required extension for {}: {}", provider.name(), e));
                                    } else {
                                        if provider == AttachProvider::MotherDuck && !attach_modal.password.trim().is_empty() {
                                            let safe_token = attach_modal.password.trim().replace('\'', "''");
                                            let _ = conn.execute(&format!("SET motherduck_token='{}';", safe_token), []);
                                        }
                                        if provider == AttachProvider::Quack && !attach_modal.password.trim().is_empty() {
                                            let safe_token = attach_modal.password.trim().replace('\'', "''");
                                            let _ = conn.execute(&format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token), []);
                                        }

                                        let mut attach_sql = format!("ATTACH '{}' AS {}", safe_path, alias);
                                        let mut opts = Vec::new();
                                        match provider {
                                            AttachProvider::SQLite => opts.push("TYPE SQLITE".to_string()),
                                            AttachProvider::PostgreSQL => opts.push("TYPE POSTGRES".to_string()),
                                            AttachProvider::MySQL => opts.push("TYPE MYSQL".to_string()),
                                            AttachProvider::MSSQL => opts.push("TYPE MSSQL".to_string()),
                                            AttachProvider::Quack => {
                                                if attach_modal.disable_ssl || attach_modal.insecure || crate::engine::should_disable_quack_ssl(&safe_path, false) {
                                                    opts.push("DISABLE_SSL true".to_string());
                                                }
                                            }
                                            _ => {}
                                        }
                                        if attach_modal.read_only {
                                            opts.push("READ_ONLY".to_string());
                                        }
                                        if !opts.is_empty() {
                                            attach_sql.push_str(&format!(" ({})", opts.join(", ")));
                                        }
                                        attach_sql.push(';');

                                        match conn.execute(&attach_sql, []) {
                                            Ok(_) => {
                                                attach_modal.is_open = false;
                                                tabs[active_tab_index].target_database = alias.clone();
                                                status_msg = format!("Attached database '{}' successfully and switched active tab context.", alias);
                                                trigger_explorer_refresh = true;
                                            }
                                            Err(e) => {
                                                attach_modal.error_msg = Some(format!("Attach Error: {}", e));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else if key.code == KeyCode::Backspace {
                        attach_modal.handle_backspace();
                    } else if let KeyCode::Char(c) = key.code {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            attach_modal.handle_char(c);
                        }
                    }
                } else if secret_modal.is_open {
                    if key.code == KeyCode::Esc {
                        secret_modal.is_open = false;
                    } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
                        secret_modal.active_field = (secret_modal.active_field + 1) % 11;
                    } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
                        secret_modal.active_field = if secret_modal.active_field == 0 { 10 } else { secret_modal.active_field - 1 };
                    } else if secret_modal.active_field == 0 && (key.code == KeyCode::Left || key.code == KeyCode::Char('h')) {
                        let prev = if secret_modal.provider_index == 0 { 4 } else { secret_modal.provider_index - 1 };
                        secret_modal.set_provider(prev);
                    } else if secret_modal.active_field == 0 && (key.code == KeyCode::Right || key.code == KeyCode::Char('l')) {
                        let next = (secret_modal.provider_index + 1) % 5;
                        secret_modal.set_provider(next);
                    } else if secret_modal.active_field >= 9 && (key.code == KeyCode::Left || key.code == KeyCode::Right) {
                        secret_modal.active_field = if secret_modal.active_field == 9 { 10 } else { 9 };
                    } else if key.code == KeyCode::Char(' ') && secret_modal.active_field == 7 {
                        secret_modal.use_ssl = !secret_modal.use_ssl;
                    } else if key.code == KeyCode::Char(' ') && secret_modal.active_field == 8 {
                        secret_modal.save_to_env = !secret_modal.save_to_env;
                    } else if key.code == KeyCode::Enter {
                        if secret_modal.active_field == 10 {
                            secret_modal.is_open = false;
                        } else if secret_modal.active_field == 7 {
                            secret_modal.use_ssl = !secret_modal.use_ssl;
                        } else if secret_modal.active_field == 8 {
                            secret_modal.save_to_env = !secret_modal.save_to_env;
                        } else {
                            let safe_name = secret_modal.name.trim().replace("'", "''");
                            let safe_key = secret_modal.key_id.trim().replace("'", "''");
                            let safe_secret = secret_modal.secret_key.trim().replace("'", "''");
                            let safe_region = secret_modal.region.trim().replace("'", "''");
                            let safe_endpoint = secret_modal.endpoint.trim().replace("'", "''");

                            if safe_name.is_empty() {
                                secret_modal.error_msg = Some("Secret name cannot be empty.".to_string());
                            } else {
                                let _ = conn.execute("INSTALL httpfs; LOAD httpfs;", []);
                                let mut clauses = vec!["TYPE S3".to_string()];
                                if !safe_key.is_empty() {
                                    clauses.push(format!("KEY_ID '{}'", safe_key));
                                }
                                if !safe_secret.is_empty() {
                                    clauses.push(format!("SECRET '{}'", safe_secret));
                                }
                                if !safe_region.is_empty() {
                                    clauses.push(format!("REGION '{}'", safe_region));
                                }
                                let mut use_ssl = secret_modal.use_ssl;
                                if !safe_endpoint.is_empty() {
                                    let clean_ep = safe_endpoint
                                        .trim_start_matches("http://")
                                        .trim_start_matches("https://")
                                        .trim_end_matches('/');
                                    clauses.push(format!("ENDPOINT '{}'", clean_ep));
                                    clauses.push("URL_STYLE 'path'".to_string());
                                    if safe_endpoint.starts_with("http://") || clean_ep.starts_with("localhost") || clean_ep.starts_with("127.0.0.1") {
                                        use_ssl = false;
                                    }
                                }
                                clauses.push(format!("USE_SSL {}", use_ssl));

                                let secret_body = clauses.join(", ");
                                let persistent_sql = format!("CREATE OR REPLACE PERSISTENT SECRET {} ({});", safe_name, secret_body);
                                let create_res = conn.execute(&persistent_sql, []).or_else(|_| {
                                    let temp_sql = format!("CREATE OR REPLACE SECRET {} ({});", safe_name, secret_body);
                                    conn.execute(&temp_sql, [])
                                });

                                match create_res {
                                    Ok(_) => {
                                        if secret_modal.save_to_env {
                                            let _ = crate::engine::save_cloud_secret_to_env(
                                                project_dir,
                                                secret_modal.key_id.trim(),
                                                secret_modal.secret_key.trim(),
                                                secret_modal.region.trim(),
                                                secret_modal.endpoint.trim(),
                                                secret_modal.bucket.trim(),
                                            );
                                        }

                                        let mut bucket = secret_modal.bucket.trim().to_string();
                                        if !bucket.is_empty() {
                                            if !bucket.starts_with("s3://") && !bucket.starts_with("r2://") && !bucket.starts_with("gcs://") {
                                                bucket = format!("s3://{}", bucket);
                                            }
                                            unsafe {
                                                std::env::set_var("DUCKPACK_S3_BUCKET", &bucket);
                                            }
                                        }

                                        secret_modal.is_open = false;
                                        status_msg = format!("Configured cloud secret '{}' successfully.", safe_name);
                                        trigger_explorer_refresh = true;
                                    }
                                    Err(e) => {
                                        secret_modal.error_msg = Some(format!("Secret Error: {}", e));
                                    }
                                }
                            }
                        }
                    } else if key.code == KeyCode::Backspace {
                        match secret_modal.active_field {
                            1 => { secret_modal.name.pop(); }
                            2 => { secret_modal.key_id.pop(); }
                            3 => { secret_modal.secret_key.pop(); }
                            4 => { secret_modal.region.pop(); }
                            5 => { secret_modal.endpoint.pop(); }
                            6 => { secret_modal.bucket.pop(); }
                            _ => {}
                        }
                    } else if let KeyCode::Char(c) = key.code {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            match secret_modal.active_field {
                                1 => { secret_modal.name.push(c); }
                                2 => { secret_modal.key_id.push(c); }
                                3 => { secret_modal.secret_key.push(c); }
                                4 => { secret_modal.region.push(c); }
                                5 => { secret_modal.endpoint.push(c); }
                                6 => { secret_modal.bucket.push(c); }
                                _ => {}
                            }
                        }
                    }
                } else if add_bucket_modal.is_open {
                    if key.code == KeyCode::Esc {
                        add_bucket_modal.is_open = false;
                    } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
                        add_bucket_modal.active_field = (add_bucket_modal.active_field + 1) % 4;
                    } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
                        add_bucket_modal.active_field = if add_bucket_modal.active_field == 0 { 3 } else { add_bucket_modal.active_field - 1 };
                    } else if add_bucket_modal.active_field >= 2 && (key.code == KeyCode::Left || key.code == KeyCode::Right) {
                        add_bucket_modal.active_field = if add_bucket_modal.active_field == 2 { 3 } else { 2 };
                    } else if key.code == KeyCode::Char(' ') && add_bucket_modal.active_field == 1 {
                        add_bucket_modal.save_to_env = !add_bucket_modal.save_to_env;
                    } else if key.code == KeyCode::Enter {
                        if add_bucket_modal.active_field == 3 {
                            add_bucket_modal.is_open = false;
                        } else if add_bucket_modal.active_field == 1 {
                            add_bucket_modal.save_to_env = !add_bucket_modal.save_to_env;
                        } else {
                            let clean_uri = add_bucket_modal.uri.trim();
                            if clean_uri.is_empty() {
                                add_bucket_modal.error_msg = Some("Bucket URI or name cannot be empty.".to_string());
                            } else {
                                let mut normalized = clean_uri.to_string();
                                if !normalized.starts_with("s3://") && !normalized.starts_with("r2://") && !normalized.starts_with("gcs://") && !normalized.starts_with("azure://") {
                                    normalized = format!("s3://{}", normalized);
                                }

                                if let Some(ref old_uri) = add_bucket_modal.initial_uri {
                                    if old_uri != &normalized {
                                        expanded_groups.remove(&format!("bucket:{}", old_uri));
                                        unsafe {
                                            let current = std::env::var("DUCKPACK_S3_BUCKETS").or_else(|_| std::env::var("DUCKPACK_S3_BUCKET")).unwrap_or_default();
                                            let parts: Vec<&str> = current.split(',')
                                                .map(|s| s.trim())
                                                .filter(|s| !s.is_empty() && *s != old_uri.as_str() && *s != old_uri.trim_start_matches("s3://"))
                                                .collect();
                                            let mut new_parts = parts;
                                            if !new_parts.contains(&normalized.as_str()) {
                                                new_parts.push(&normalized);
                                            }
                                            let joined = new_parts.join(",");
                                            std::env::set_var("DUCKPACK_S3_BUCKETS", &joined);
                                            std::env::set_var("DUCKPACK_S3_BUCKET", &joined);
                                        }
                                    }
                                }

                                if add_bucket_modal.save_to_env {
                                    let _ = crate::engine::append_cloud_bucket_to_env(project_dir, &normalized);
                                }

                                unsafe {
                                    let current = std::env::var("DUCKPACK_S3_BUCKETS").or_else(|_| std::env::var("DUCKPACK_S3_BUCKET")).unwrap_or_default();
                                    let new_var = if current.is_empty() {
                                        normalized.clone()
                                    } else if !current.split(',').any(|b| b.trim() == normalized) {
                                        format!("{},{}", current, normalized)
                                    } else {
                                        current
                                    };
                                    std::env::set_var("DUCKPACK_S3_BUCKETS", &new_var);
                                    std::env::set_var("DUCKPACK_S3_BUCKET", &new_var);
                                }

                                let bucket_key = format!("bucket:{}", normalized);
                                expanded_groups.insert("Secrets & Cloud Buckets".to_string());
                                expanded_groups.insert(bucket_key);

                                add_bucket_modal.is_open = false;
                                status_msg = format!("Added bucket '{}' to explorer.", normalized);
                                trigger_explorer_refresh = true;
                            }
                        }
                    } else if key.code == KeyCode::Backspace && add_bucket_modal.active_field == 0 {
                        add_bucket_modal.uri.pop();
                    } else if let KeyCode::Char(c) = key.code {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && add_bucket_modal.active_field == 0 {
                            add_bucket_modal.uri.push(c);
                        }
                    }
                } else if db_picker_modal.is_open {
                    if key.code == KeyCode::Esc {
                        db_picker_modal.is_open = false;
                    } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                        db_picker_modal.prev();
                    } else if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                        db_picker_modal.next(meta.databases.len());
                    } else if key.code == KeyCode::Enter {
                        if let Some(target_db) = meta.databases.get(db_picker_modal.selected_index) {
                            let db_name = target_db.name.clone();
                            tabs[active_tab_index].target_database = db_name.clone();
                            let _ = conn.execute(&format!("USE \"{}\";", db_name.replace('"', "\"\"")), []);
                            meta.active_database = db_name.clone();
                            for db in &mut meta.databases {
                                db.is_active = db.name == db_name;
                            }
                            status_msg = format!("Target database switched to '{}' for active tab.", db_name);
                        }
                        db_picker_modal.is_open = false;
                    }
                } else if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    attach_modal.reset();
                } else if key.code == KeyCode::Char('k') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    secret_modal.reset();
                } else if show_help || show_schema_visualizer {
                    if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') || key.code == KeyCode::Char('?') {
                        show_help = false;
                        show_schema_visualizer = false;
                    } else if show_schema_visualizer {
                        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            schema_scroll = (schema_scroll + 1).min(max_schema_scroll);
                        } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            schema_scroll = schema_scroll.saturating_sub(1);
                        }
                    }
                } else {
                    if key.code == KeyCode::F(2) {
                        db_picker_modal.open_for_current(&tabs[active_tab_index].target_database, &meta.databases);
                        continue;
                    }

                    if key.code == KeyCode::Char('?') {
                    show_help = true;
                    continue;
                }

                if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    show_schema_visualizer = true;
                    schema_scroll = 0;
                    schema_ascii_tree = generate_schema_visualizer(conn, &mut os_clipboard, is_quack).unwrap_or_else(|e| format!("Failed to generate schema visualizer: {}", e));
                    continue;
                }

                if key.code == KeyCode::F(3) {
                    explorer_mode = ExplorerMode::Database;
                    explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                    explorer_state.select(Some(0));
                    status_msg = "Switched to Database Catalog view".to_string();
                    continue;
                }

                if key.code == KeyCode::F(4) {
                    explorer_mode = ExplorerMode::Project;
                    let (b, st) = get_git_branch_and_status(project_dir);
                    git_branch = b;
                    git_status = st;
                    explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
                    explorer_state.select(Some(0));
                    status_msg = "Switched to Project Files view".to_string();
                    continue;
                }

                if key.code == KeyCode::F(6) || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                    let tab_cat = tabs.get(active_tab_index).map(|t| t.target_database.as_str()).filter(|c| !c.is_empty() && *c != "memory");
                    match compute_ide_diff_for_catalog(project_dir, conn, tab_cat) {
                        Ok(diff) => {
                            in_ide_diff_modal.reset();
                            in_ide_diff_modal.diff_result = Some(diff);
                            in_ide_diff_modal.is_open = true;
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            status_msg = format!("⚠️ Diff Failed: {}", err_str);
                            handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                        }
                    }
                    continue;
                }

                if key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    match crate::engine::build_shadow_db(project_dir) {
                        Ok(_) => {
                            status_msg = "✓ Project validated successfully (0 syntax or schema errors).".to_string();
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            status_msg = format!("⚠️ Validation Error: {}", err_str);
                            handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                        }
                    }
                    continue;
                }

                if key.code == KeyCode::F(5) || (key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                    execute_query = true;
                }

                // Global Alt shortcuts for explorer resizing
                if key.modifiers.contains(KeyModifiers::ALT) {
                    if key.code == KeyCode::Char('w') || key.code == KeyCode::Char('W') {
                        explorer_width_percent = match explorer_width_percent {
                            p if p < 30 => 35,
                            p if p < 45 => 50,
                            _ => 25,
                        };
                        status_msg = format!("Explorer width preset: {}%", explorer_width_percent);
                        continue;
                    } else if key.code == KeyCode::Char(']') || key.code == KeyCode::Char('+') || key.code == KeyCode::Char('=') {
                        explorer_width_percent = (explorer_width_percent + 5).min(60);
                        status_msg = format!("Explorer width: {}%", explorer_width_percent);
                        continue;
                    } else if key.code == KeyCode::Char('[') || key.code == KeyCode::Char('-') {
                        explorer_width_percent = explorer_width_percent.saturating_sub(5).max(15);
                        status_msg = format!("Explorer width: {}%", explorer_width_percent);
                        continue;
                    }
                }

                // Global quit confirmation prompt
                if key.code == KeyCode::Esc {
                    exit_confirm_modal.open();
                    continue;
                }

                // Global tab navigation (forward)
                if key.code == KeyCode::Tab {
                    focus = match focus {
                        FocusPane::Explorer => FocusPane::Editor,
                        FocusPane::Editor => FocusPane::Results,
                        FocusPane::Results => FocusPane::Explorer,
                    };
                    continue;
                }

                // Global tab navigation (backward)
                if key.code == KeyCode::BackTab {
                    focus = match focus {
                        FocusPane::Explorer => FocusPane::Results,
                        FocusPane::Editor => FocusPane::Explorer,
                        FocusPane::Results => FocusPane::Editor,
                    };
                    continue;
                }

                // Save query
                if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    save_query = true;
                }

                // Export query
                if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    export_query = true;
                }

                // Format query
                if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    format_query = true;
                }

                // New Tab
                if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    new_tab = true;
                }
                
                // Close Tab
                if key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    tabs.remove(active_tab_index);
                    if tabs.is_empty() {
                        tabs.push(EditorTab::new(meta.active_database.clone()));
                    }
                    if active_tab_index >= tabs.len() {
                        active_tab_index = tabs.len() - 1;
                    }
                    status_msg = "Closed tab".to_string();
                }

                // Next Tab
                if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    active_tab_index = (active_tab_index + 1) % tabs.len();
                }

                // Prev Tab
                if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if active_tab_index == 0 {
                        active_tab_index = tabs.len() - 1;
                    } else {
                        active_tab_index -= 1;
                    }
                }

                // Copy to OS Clipboard
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) && focus == FocusPane::Editor {
                    let active_tab = &mut tabs[active_tab_index];
                    let text_to_copy = if active_tab.textarea.selection_range().is_some() {
                        active_tab.textarea.copy();
                        active_tab.textarea.yank_text().to_string()
                    } else {
                        // If no selection, copy the current line
                        let cursor = active_tab.textarea.cursor();
                        if cursor.0 < active_tab.textarea.lines().len() {
                            format!("{}\n", active_tab.textarea.lines()[cursor.0])
                        } else {
                            active_tab.textarea.copy();
                            active_tab.textarea.yank_text().to_string()
                        }
                    };
                    if !text_to_copy.is_empty() {
                        if set_clipboard_text(&text_to_copy, &mut os_clipboard) {
                            status_msg = "Copied to clipboard".to_string();
                        }
                    }
                    continue;
                }

                // Paste from OS Clipboard
                if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) && focus == FocusPane::Editor {
                    if let Some(text) = get_clipboard_text(&mut os_clipboard) {
                        let active_tab = &mut tabs[active_tab_index];
                        active_tab.textarea.insert_str(&text);
                        status_msg = "Pasted from clipboard".to_string();
                    }
                    continue;
                }

                // Pane-specific events
                match focus {
                    FocusPane::Explorer => {
                        if key.code == KeyCode::Up {
                            if let Some(selected) = explorer_state.selected() {
                                if selected > 0 {
                                    explorer_state.select(Some(selected - 1));
                                }
                            }
                        } else if key.code == KeyCode::Down {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len().saturating_sub(1) {
                                    explorer_state.select(Some(selected + 1));
                                }
                            }
                        } else if key.code == KeyCode::Left || key.code == KeyCode::Char('h') {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                explorer_horizontal_scroll = explorer_horizontal_scroll.saturating_sub(10);
                            } else {
                                explorer_horizontal_scroll = explorer_horizontal_scroll.saturating_sub(4);
                            }
                        } else if key.code == KeyCode::Right || key.code == KeyCode::Char('l') {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                explorer_horizontal_scroll += 10;
                            } else {
                                explorer_horizontal_scroll += 4;
                            }
                        } else if key.code == KeyCode::Home {
                            explorer_horizontal_scroll = 0;
                        } else if key.code == KeyCode::Char('w') || key.code == KeyCode::Char('W') {
                            explorer_width_percent = match explorer_width_percent {
                                p if p < 30 => 35,
                                p if p < 45 => 50,
                                _ => 25,
                            };
                            status_msg = format!("Explorer width preset: {}%", explorer_width_percent);
                        } else if key.code == KeyCode::Char('+') || key.code == KeyCode::Char('=') || key.code == KeyCode::Char(']') {
                            explorer_width_percent = (explorer_width_percent + 5).min(60);
                            status_msg = format!("Explorer width: {}%", explorer_width_percent);
                        } else if key.code == KeyCode::Char('-') || key.code == KeyCode::Char('[') {
                            explorer_width_percent = explorer_width_percent.saturating_sub(5).max(15);
                            status_msg = format!("Explorer width: {}%", explorer_width_percent);
                        } else if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                            trigger_explorer_action = true;
                        } else if key.code == KeyCode::Char('1') {
                            explorer_mode = ExplorerMode::Database;
                            explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Database Catalog view".to_string();
                        } else if key.code == KeyCode::Char('2') {
                            explorer_mode = ExplorerMode::Project;
                            let (b, st) = get_git_branch_and_status(project_dir);
                            git_branch = b;
                            git_status = st;
                            explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Project Files view".to_string();
                        } else if key.code == KeyCode::Char('a') || key.code == KeyCode::Char('n') {
                            if explorer_mode == ExplorerMode::Project {
                                new_asset_modal.reset();
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        match &explorer_items[selected].node_type {
                                            NodeType::Group => {
                                                let name = &explorer_items[selected].name;
                                                if name.contains("tables") { new_asset_modal.category_index = 0; }
                                                else if name.contains("views") { new_asset_modal.category_index = 1; }
                                                else if name.contains("macros") { new_asset_modal.category_index = 2; }
                                                else if name.contains("triggers") { new_asset_modal.category_index = 3; }
                                                else if name.contains("schemas") { new_asset_modal.category_index = 4; }
                                                else if name.contains("queries") { new_asset_modal.category_index = 5; }
                                                else if name.contains("seeds") { new_asset_modal.category_index = 6; }
                                            }
                                            NodeType::ProjectFile { category, .. } => {
                                                match category {
                                                    ProjectCategory::Table => new_asset_modal.category_index = 0,
                                                    ProjectCategory::View => new_asset_modal.category_index = 1,
                                                    ProjectCategory::Macro => new_asset_modal.category_index = 2,
                                                    ProjectCategory::Trigger => new_asset_modal.category_index = 3,
                                                    ProjectCategory::Schema => new_asset_modal.category_index = 4,
                                                    ProjectCategory::Query => new_asset_modal.category_index = 5,
                                                    ProjectCategory::Seed => new_asset_modal.category_index = 6,
                                                    _ => {}
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        match &explorer_items[selected].node_type {
                                            NodeType::Group if explorer_items[selected].name == "Secrets & Cloud Buckets" => {
                                                secret_modal.reset();
                                            }
                                            NodeType::AddSecretAction | NodeType::CloudSecret { .. } => {
                                                secret_modal.reset();
                                            }
                                            NodeType::AddBucketAction | NodeType::CloudBucket { .. } | NodeType::CloudFile { .. } => {
                                                add_bucket_modal.reset();
                                            }
                                            _ => {
                                                attach_modal.reset();
                                            }
                                        }
                                    } else {
                                        attach_modal.reset();
                                    }
                                } else {
                                    attach_modal.reset();
                                }
                            }
                        } else if key.code == KeyCode::Char('v') || key.code == KeyCode::Char('V') {
                            if explorer_mode == ExplorerMode::Project {
                                match crate::engine::build_shadow_db(project_dir) {
                                    Ok(_) => {
                                        status_msg = "✓ Project validated successfully (0 syntax or schema errors).".to_string();
                                    }
                                    Err(e) => {
                                        let err_str = e.to_string();
                                        status_msg = format!("⚠️ Validation Error: {}", err_str);
                                        handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                                    }
                                }
                            }
                        } else if key.code == KeyCode::Char('r') || key.code == KeyCode::Char('R') {
                            if explorer_mode == ExplorerMode::Project {
                                let (b, st) = get_git_branch_and_status(project_dir);
                                git_branch = b;
                                git_status = st;
                                explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
                                status_msg = "Project files and git status refreshed.".to_string();
                            } else {
                                refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                trigger_explorer_refresh = true;
                                status_msg = "Explorer refreshed.".to_string();
                            }
                        } else if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('s') {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len() {
                                    let item_type = &explorer_items[selected].node_type;
                                    let item_name = &explorer_items[selected].name;
                                    match item_type {
                                        NodeType::Table { database, schema, is_temporary } | NodeType::View { database, schema, is_temporary } => {
                                            let db_name = database.as_deref().unwrap_or(&meta.active_database);
                                            let schema_name = schema.as_deref().unwrap_or("main");

                                            let quote_part = |s: &str| -> String {
                                                if s.chars().any(|c| !c.is_alphanumeric() && c != '_') {
                                                    format!("\"{}\"", s.replace('"', "\"\""))
                                                } else {
                                                    s.to_string()
                                                }
                                            };
                                            let quote_schema_path = |s: &str| -> String {
                                                s.split('.').map(quote_part).collect::<Vec<_>>().join(".")
                                            };
                                            let quoted_item = quote_part(item_name);
                                            let quoted_schema = quote_schema_path(schema_name);
                                            let quoted_db = quote_part(db_name);

                                            let qualified_sql = if *is_temporary {
                                                format!("SELECT * FROM {} LIMIT 500;", quoted_item)
                                            } else if is_quack && db_name != meta.active_database && db_name != "memory" && db_name != "target_db" {
                                                if schema_name == "main" {
                                                    format!("-- @remote\nSELECT * FROM {}.{} LIMIT 500;", quoted_db, quoted_item)
                                                } else {
                                                    format!("-- @remote\nSELECT * FROM {}.{}.{} LIMIT 500;", quoted_db, quoted_schema, quoted_item)
                                                }
                                            } else if meta.databases.len() > 1 {
                                                if schema_name == "main" {
                                                    format!("SELECT * FROM {}.{} LIMIT 500;", quoted_db, quoted_item)
                                                } else {
                                                    format!("SELECT * FROM {}.{}.{} LIMIT 500;", quoted_db, quoted_schema, quoted_item)
                                                }
                                            } else if db_name == meta.active_database && schema_name == "main" {
                                                format!("SELECT * FROM {} LIMIT 500;", quoted_item)
                                            } else if schema_name == "main" {
                                                format!("SELECT * FROM {}.{} LIMIT 500;", quoted_db, quoted_item)
                                            } else {
                                                format!("SELECT * FROM {}.{}.{} LIMIT 500;", quoted_db, quoted_schema, quoted_item)
                                            };

                                            let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                            if !current_is_empty {
                                                tabs.push(EditorTab::new(db_name.to_string()));
                                                active_tab_index = tabs.len() - 1;
                                            } else {
                                                tabs[active_tab_index].target_database = db_name.to_string();
                                            }

                                            tabs[active_tab_index].textarea = TextArea::from(vec![qualified_sql]);
                                            tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                            focus = FocusPane::Editor;
                                            execute_query = true;
                                            let display_table = if schema_name == "main" { item_name.clone() } else { format!("{}.{}", schema_name, item_name) };
                                            status_msg = format!("Queried table '{}'", display_table);
                                        }
                                        NodeType::CloudFile { path, is_dir } if !*is_dir => {
                                            let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
                                            let read_fn = match ext.as_str() {
                                                "parquet" => "read_parquet",
                                                "csv" => "read_csv",
                                                "json" | "jsonl" | "ndjson" => "read_json",
                                                _ => "read_parquet",
                                            };
                                            let query = format!("SELECT * FROM {}('{}') LIMIT 500;", read_fn, path);
                                            let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                            if !current_is_empty {
                                                tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                                active_tab_index = tabs.len() - 1;
                                            }
                                            tabs[active_tab_index].textarea = TextArea::from(vec![query]);
                                            tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                            focus = FocusPane::Editor;
                                            execute_query = true;
                                            let fname = path.split('/').last().unwrap_or(path);
                                            status_msg = format!("Queried cloud file '{}'", fname);
                                        }
                                        NodeType::CloudSecret { name, .. } => {
                                            let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                            if !current_is_empty {
                                                tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                                active_tab_index = tabs.len() - 1;
                                            }
                                            let query = format!("SELECT * FROM duckdb_secrets() WHERE name = '{}';", name);
                                            tabs[active_tab_index].textarea = TextArea::from(vec![query]);
                                            tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                            focus = FocusPane::Editor;
                                            execute_query = true;
                                            status_msg = format!("Inspecting secret '{}'", name);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        } else if key.code == KeyCode::Char('d') || key.code == KeyCode::Delete {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len() {
                                    match &explorer_items[selected].node_type {
                                        NodeType::Database { is_active, .. } => {
                                            if *is_active {
                                                status_msg = "⚠️ Cannot detach the primary active database.".to_string();
                                            } else {
                                                let db_name = explorer_items[selected].name.strip_prefix("db:").unwrap_or(&explorer_items[selected].name);
                                                match conn.execute(&format!("DETACH {};", db_name), []) {
                                                    Ok(_) => {
                                                        status_msg = format!("Detached database '{}' successfully.", db_name);
                                                        trigger_explorer_refresh = true;
                                                    }
                                                    Err(e) => {
                                                        status_msg = format!("Failed to detach '{}': {}", db_name, e);
                                                    }
                                                }
                                            }
                                        }
                                        NodeType::CloudSecret { name, .. } => {
                                            match conn.execute(&format!("DROP SECRET IF EXISTS {};", name), []) {
                                                Ok(_) => {
                                                    status_msg = format!("Dropped secret '{}' successfully.", name);
                                                    trigger_explorer_refresh = true;
                                                }
                                                Err(e) => {
                                                    status_msg = format!("Failed to drop secret '{}': {}", name, e);
                                                }
                                            }
                                        }
                                        NodeType::CloudBucket { bucket } => {
                                            let current = std::env::var("DUCKPACK_S3_BUCKETS").or_else(|_| std::env::var("DUCKPACK_S3_BUCKET")).unwrap_or_default();
                                            let parts: Vec<&str> = current.split(',')
                                                .map(|s| s.trim())
                                                .filter(|s| !s.is_empty() && *s != bucket.as_str() && *s != bucket.trim_start_matches("s3://"))
                                                .collect();
                                            let new_val = parts.join(",");
                                            unsafe {
                                                std::env::set_var("DUCKPACK_S3_BUCKETS", &new_val);
                                                std::env::set_var("DUCKPACK_S3_BUCKET", &new_val);
                                            }
                                            status_msg = format!("Removed bucket '{}' from session view.", bucket);
                                            trigger_explorer_refresh = true;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        } else if key.code == KeyCode::Char('e') || key.code == KeyCode::Char('E') {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len() {
                                    match &explorer_items[selected].node_type {
                                        NodeType::CloudSecret { name, .. } => {
                                            populate_secret_modal_for_edit(conn, project_dir, name, &mut secret_modal);
                                            status_msg = format!("Editing cloud secret '{}'", name);
                                        }
                                        NodeType::CloudBucket { bucket } => {
                                            add_bucket_modal.is_open = true;
                                            add_bucket_modal.initial_uri = Some(bucket.clone());
                                            add_bucket_modal.uri = bucket.clone();
                                            add_bucket_modal.active_field = 0;
                                            add_bucket_modal.error_msg = None;
                                            status_msg = format!("Editing bucket URI '{}'", bucket);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    FocusPane::Editor => {
                        if key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            let cursor = tabs[active_tab_index].textarea.cursor();
                            if cursor.0 < tabs[active_tab_index].textarea.lines().len() {
                                let line = &tabs[active_tab_index].textarea.lines()[cursor.0];
                                if cursor.1 <= line.chars().count() {
                                    // Need to convert char index to byte index for slice slicing, but line is a String.
                                    // tui-textarea's cursor.1 is a char index.
                                    let char_idx = cursor.1;
                                    let prefix: String = line.chars().take(char_idx).collect();
                                    
                                    let word_start = prefix.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.').map(|i| i + 1).unwrap_or(0);
                                    let current_word = &prefix[word_start..];

                                    if !current_word.is_empty() {
                                        let mut is_cycling = false;
                                        if !autocomplete_matches.is_empty() && current_word == autocomplete_matches[autocomplete_index] {
                                            is_cycling = true;
                                        }

                                        if is_cycling {
                                            autocomplete_index = (autocomplete_index + 1) % autocomplete_matches.len();
                                        } else {
                                            autocomplete_original_word = current_word.to_string();
                                            let mut matches: Vec<String> = dictionary.iter()
                                                .filter(|d| d.to_lowercase().starts_with(&autocomplete_original_word.to_lowercase()))
                                                .cloned().collect();
                                            let target_db = &tabs[active_tab_index].target_database;
                                            if !target_db.is_empty() {
                                                let target_prefix = format!("{}.", target_db.to_lowercase());
                                                let target_entities: HashSet<String> = if let Some(d) = meta.databases.iter().find(|d| d.name == *target_db) {
                                                    let mut set = HashSet::new();
                                                    set.insert(d.name.to_lowercase());
                                                    for t in &d.tables {
                                                        set.insert(t.name.to_lowercase());
                                                        set.insert(t.display_name().to_lowercase());
                                                    }
                                                    for v in &d.views {
                                                        set.insert(v.name.to_lowercase());
                                                        set.insert(v.display_name().to_lowercase());
                                                    }
                                                    set
                                                } else {
                                                    HashSet::new()
                                                };

                                                matches.sort_by(|a, b| {
                                                    let a_low = a.to_lowercase();
                                                    let b_low = b.to_lowercase();
                                                    let a_score = if a_low.starts_with(&target_prefix) || target_entities.contains(&a_low) { 0 } else { 1 };
                                                    let b_score = if b_low.starts_with(&target_prefix) || target_entities.contains(&b_low) { 0 } else { 1 };
                                                    a_score.cmp(&b_score).then_with(|| a.len().cmp(&b.len())).then_with(|| a.cmp(b))
                                                });
                                            }
                                            autocomplete_matches = matches;
                                            autocomplete_index = 0;
                                        }

                                        if !autocomplete_matches.is_empty() {
                                            let match_str = &autocomplete_matches[autocomplete_index];
                                            for _ in 0..current_word.chars().count() {
                                                tabs[active_tab_index].textarea.delete_char();
                                            }
                                            tabs[active_tab_index].textarea.insert_str(match_str);
                                            tabs[active_tab_index].is_dirty = true;
                                            status_msg = format!("Autocomplete: {} ({}/{})", match_str, autocomplete_index + 1, autocomplete_matches.len());
                                        } else {
                                            status_msg = format!("No autocomplete matches for '{}'", autocomplete_original_word);
                                        }
                                    }
                                }
                            }
                        } else {
                            if tabs[active_tab_index].textarea.input(key) {
                                tabs[active_tab_index].is_dirty = true;
                            }
                        }
                    }
                    FocusPane::Results => {
                        let active_tab = &mut tabs[active_tab_index];
                        if key.code == KeyCode::Up {
                            if let Some(selected) = active_tab.results_state.selected() {
                                if selected > 0 {
                                    active_tab.results_state.select(Some(selected - 1));
                                }
                            }
                        } else if key.code == KeyCode::Down {
                            let len = active_tab.query_results.len();
                            if len > 0 {
                                let selected = active_tab.results_state.selected().unwrap_or(0);
                                if selected < len.saturating_sub(1) {
                                    active_tab.results_state.select(Some(selected + 1));
                                } else if active_tab.results_state.selected().is_none() {
                                    active_tab.results_state.select(Some(0));
                                }
                            }
                        } else if key.code == KeyCode::PageDown {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                if active_tab.query_results.len() == active_tab.pagination_limit {
                                    trigger_pagination = Some(1);
                                }
                            } else {
                                let len = active_tab.query_results.len();
                                if len > 0 {
                                    let selected = active_tab.results_state.selected().unwrap_or(0);
                                    let new_sel = (selected + 10).min(len.saturating_sub(1));
                                    active_tab.results_state.select(Some(new_sel));
                                }
                            }
                        } else if key.code == KeyCode::Char('n') || key.code == KeyCode::Char('N') {
                            if active_tab.query_results.len() == active_tab.pagination_limit {
                                trigger_pagination = Some(1);
                            }
                        } else if key.code == KeyCode::Char('p') || key.code == KeyCode::Char('P') {
                            trigger_pagination = Some(-1);
                        } else if key.code == KeyCode::PageUp {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                trigger_pagination = Some(-1);
                            } else {
                                if let Some(selected) = active_tab.results_state.selected() {
                                    let new_sel = selected.saturating_sub(10);
                                    active_tab.results_state.select(Some(new_sel));
                                }
                            }
                        } else if key.code == KeyCode::Left {
                            if key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::CONTROL) {
                                active_tab.horizontal_scroll = active_tab.horizontal_scroll.saturating_sub(5);
                            } else {
                                active_tab.horizontal_scroll = active_tab.horizontal_scroll.saturating_sub(1);
                            }
                        } else if key.code == KeyCode::Right {
                            let max_col = active_tab.column_names.len().saturating_sub(1);
                            if key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::CONTROL) {
                                active_tab.horizontal_scroll = (active_tab.horizontal_scroll + 5).min(max_col);
                            } else if active_tab.horizontal_scroll < max_col {
                                active_tab.horizontal_scroll += 1;
                            }
                        } else if key.code == KeyCode::Home {
                            active_tab.horizontal_scroll = 0;
                        } else if key.code == KeyCode::End {
                            active_tab.horizontal_scroll = active_tab.column_names.len().saturating_sub(1);
                        }
                    }
                }
                }
            }
            _ => {}
        }

        if new_tab {
            tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
            active_tab_index = tabs.len() - 1;
            status_msg = "New tab opened".to_string();
        }

        if close_tab {
            tabs.remove(active_tab_index);
            if tabs.is_empty() {
                tabs.push(EditorTab::new(meta.active_database.clone()));
            }
            if active_tab_index >= tabs.len() {
                active_tab_index = tabs.len() - 1;
            }
            status_msg = "Closed tab".to_string();
        }

        if format_query {
            let query = tabs[active_tab_index].textarea.lines().join("\n");
            let formatted = sqlformat::format(&query, &sqlformat::QueryParams::None, &sqlformat::FormatOptions::default());
            tabs[active_tab_index].textarea = TextArea::from(formatted.lines().map(|s| s.to_string()).collect::<Vec<_>>());
            tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
            status_msg = "Formatted SQL".to_string();
            tabs[active_tab_index].is_dirty = true;
        }

        if save_query {
            let content = tabs[active_tab_index].textarea.lines().join("\n");
            if let Some(path) = tabs[active_tab_index].active_file.clone() {
                if let Err(e) = std::fs::write(&path, content) {
                    status_msg = format!("Error saving file: {}", e);
                } else {
                    tabs[active_tab_index].is_dirty = false;
                    status_msg = format!("Saved {}", path.display());
                    trigger_explorer_refresh = true;
                }
            } else {
                let name = format!("query_{}.sql", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
                let path = queries_dir.join(&name);
                if let Err(e) = std::fs::write(&path, content) {
                    status_msg = format!("Error saving new file: {}", e);
                } else {
                    tabs[active_tab_index].active_file = Some(path.clone());
                    tabs[active_tab_index].is_dirty = false;
                    status_msg = format!("Saved as {}", name);
                    trigger_explorer_refresh = true;
                }
            }
        }

        if export_query {
            let active_tab = &tabs[active_tab_index];
            if active_tab.query_results.is_empty() {
                status_msg = "No results to export!".to_string();
            } else {
                let name = format!("export_{}.csv", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
                let path = queries_dir.join(&name);
                let mut csv_content = String::new();
                csv_content.push_str(&active_tab.column_names.join(","));
                csv_content.push('\n');
                for row in &active_tab.query_results {
                    let escaped_row: Vec<String> = row.iter().map(|c| {
                        if c.contains(',') || c.contains('"') || c.contains('\n') {
                            format!("\"{}\"", c.replace("\"", "\"\""))
                        } else {
                            c.clone()
                        }
                    }).collect();
                    csv_content.push_str(&escaped_row.join(","));
                    csv_content.push('\n');
                }
                if let Err(e) = std::fs::write(&path, csv_content) {
                    status_msg = format!("Error exporting CSV: {}", e);
                } else {
                    status_msg = format!("Results exported to {}", name);
                }
            }
        }

        if execute_query || trigger_pagination.is_some() {
            let active_tab = &mut tabs[active_tab_index];
            if !active_tab.target_database.is_empty() {
                let _ = conn.execute(&format!("USE \"{}\";", active_tab.target_database.replace('"', "\"\"")), []);
                meta.active_database = active_tab.target_database.clone();
                for db in &mut meta.databases {
                    db.is_active = db.name == active_tab.target_database;
                }
            }
            
            let query_to_execute = if let Some(dir) = trigger_pagination {
                if let Some(bq) = &active_tab.base_query {
                    let new_offset = (active_tab.pagination_offset as i64 + (dir as i64 * active_tab.pagination_limit as i64)).max(0) as usize;
                    active_tab.pagination_offset = new_offset;
                    bq.clone()
                } else {
                    String::new()
                }
            } else {
                let mut query = if let Some(((start_row, start_col), (end_row, end_col))) = active_tab.textarea.selection_range() {
                    let lines = active_tab.textarea.lines();
                    let mut selected_text = Vec::new();
                    for row in start_row..=end_row {
                        if row >= lines.len() { continue; }
                        let chars: Vec<char> = lines[row].chars().collect();
                        let s = if row == start_row { start_col } else { 0 };
                        let e = if row == end_row { end_col } else { chars.len() };
                        let s = s.min(chars.len());
                        let e = e.min(chars.len());
                        if s <= e {
                            selected_text.push(chars[s..e].iter().collect::<String>());
                        }
                    }
                    selected_text.join("\n")
                } else {
                    active_tab.textarea.lines().join("\n")
                };
                if query.is_empty() {
                    query = active_tab.textarea.lines().join("\n");
                }
                active_tab.pagination_offset = 0;
                active_tab.base_query = Some(query.clone());
                query
            };

            if query_to_execute.trim().is_empty() {
                if trigger_pagination.is_none() {
                    status_msg = "Error: Query is empty".to_string();
                }
            } else {
                active_tab.query_results.clear();
                active_tab.column_names.clear();
                active_tab.horizontal_scroll = 0;
                
                // Reset syntax highlighting
                active_tab.textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                active_tab.textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

                let is_explicit_remote = is_remote_directive_query(&query_to_execute);
                let (exec_query, is_paginatable, is_mutation) = if is_explicit_remote {
                    let (eq, is_pag) = wrap_remote_query(&query_to_execute);
                    (eq, is_pag, false)
                } else {
                    let mut clean_lower = query_to_execute.trim().to_lowercase();
                    loop {
                        if clean_lower.starts_with("--") {
                            if let Some(idx) = clean_lower.find('\n') {
                                clean_lower = clean_lower[idx..].trim_start().to_string();
                            } else {
                                clean_lower = String::new();
                            }
                        } else if clean_lower.starts_with("/*") {
                            if let Some(idx) = clean_lower.find("*/") {
                                clean_lower = clean_lower[idx+2..].trim_start().to_string();
                            } else {
                                clean_lower = String::new();
                            }
                        } else {
                            break;
                        }
                    }
                    let is_pag = clean_lower.starts_with("select") || clean_lower.starts_with("with") || clean_lower.starts_with("values") || clean_lower.starts_with("from");
                    let is_mut = clean_lower.starts_with("insert")
                        || clean_lower.starts_with("update")
                        || clean_lower.starts_with("delete")
                        || clean_lower.starts_with("create")
                        || clean_lower.starts_with("drop")
                        || clean_lower.starts_with("alter")
                        || clean_lower.starts_with("truncate")
                        || clean_lower.starts_with("attach")
                        || clean_lower.starts_with("detach")
                        || clean_lower.starts_with("use")
                        || clean_lower.starts_with("set")
                        || clean_lower.starts_with("reset")
                        || clean_lower.starts_with("copy")
                        || clean_lower.starts_with("checkpoint")
                        || clean_lower.starts_with("vacuum")
                        || clean_lower.starts_with("install")
                        || clean_lower.starts_with("load");
                    (query_to_execute.clone(), is_pag, is_mut)
                };

                if is_mutation {
                    active_tab.is_paginated = false;
                    let start_time = std::time::Instant::now();
                    let mut exec_batch_res = conn.execute_batch(&exec_query);
                    if let Err(ref e) = exec_batch_res {
                        let err_str = e.to_string();
                        let lower_exec = exec_query.trim().to_lowercase();
                        if let Some(ext) = extract_autoload_extension_name(&err_str) {
                            if crate::engine::ensure_extension_loaded(conn, &ext).is_ok() {
                                exec_batch_res = Ok(());
                            }
                        } else if lower_exec.starts_with("install ") {
                            let after = lower_exec["install ".len()..].trim();
                            let ext = after.trim_matches(|c| c == '\'' || c == '"' || c == ';').split_whitespace().next().unwrap_or("");
                            if !ext.is_empty() && !ext.starts_with("http") {
                                if crate::engine::ensure_extension_loaded(conn, ext).is_ok() {
                                    exec_batch_res = Ok(());
                                }
                            }
                        }
                    }
                    match exec_batch_res {
                        Ok(()) => {
                            active_tab.query_results.clear();
                            active_tab.column_names.clear();
                            let elapsed = start_time.elapsed();

                            trigger_explorer_refresh = true;
                            status_msg = format!("Statement executed successfully in {:.2?}.", elapsed);

                            let history_path = queries_dir.join(".history.sql");
                            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                            let history_entry = format!("-- Executed at {} (Took {:.2?})\n{};\n\n", timestamp, elapsed, query_to_execute.trim());
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path) {
                                let _ = f.write_all(history_entry.as_bytes());
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                            let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                            status_msg = format!("Execution Error: {}{}", e, diag);
                        }
                    }
                } else {
                    let final_query = if is_paginatable {
                        active_tab.is_paginated = true;
                        let clean_query = exec_query.trim().strip_suffix(';').unwrap_or(exec_query.trim());
                        format!("SELECT * FROM ({}) LIMIT {} OFFSET {}", clean_query, active_tab.pagination_limit, active_tab.pagination_offset)
                    } else {
                        active_tab.is_paginated = false;
                        exec_query.clone()
                    };

                    let mut prepare_res = conn.prepare(&final_query);
                    if let Err(ref e) = prepare_res {
                        let err_str = e.to_string();
                        if let Some(ext) = extract_autoload_extension_name(&err_str) {
                            if crate::engine::ensure_extension_loaded(conn, &ext).is_ok() {
                                prepare_res = conn.prepare(&final_query);
                            }
                        }
                    }

                    match prepare_res {
                    Ok(mut stmt) => {
                        let mut exec_rows = Vec::new();
                        let mut count = 0;

                        let start_time = std::time::Instant::now();
                        match stmt.query([]) {
                            Ok(mut rows) => {
                                let mut cols = Vec::new();
                                if let Some(stmt_ref) = rows.as_ref() {
                                    for col in stmt_ref.column_names() {
                                        cols.push(col.to_string());
                                    }
                                }
                                active_tab.column_names = cols;

                                while let Ok(Some(row)) = rows.next() {
                                    count += 1;
                                    let mut str_row = Vec::new();
                                    for i in 0..active_tab.column_names.len() {
                                        let val: String = match row.get(i) {
                                            Ok(v) => format_duckdb_value(v),
                                            Err(_) => "NULL".to_string(),
                                        };
                                        str_row.push(val);
                                    }
                                    exec_rows.push(str_row);
                                }
                                active_tab.query_results = exec_rows;
                                active_tab.results_state.select(Some(0));
                                active_tab.horizontal_scroll = 0;
                                let elapsed = start_time.elapsed();

                                let lower = query_to_execute.to_lowercase();
                                let is_catalog_mutation = lower.contains("attach ")
                                    || lower.contains("detach ")
                                    || lower.contains("create ")
                                    || lower.contains("drop ")
                                    || lower.contains("alter ")
                                    || lower.contains("use ");

                                if is_explicit_remote {
                                    if is_catalog_mutation {
                                        refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                        trigger_explorer_refresh = true;
                                    }
                                    let hint = " (Hint: '-- @remote' is deprecated in DuckDB v2.0; queries now execute natively via CONNECT target_db.)";
                                    if count > 0 {
                                        status_msg = format!("Remote statement executed successfully. Returned {} rows in {:.2?}.{}", count, elapsed, hint);
                                    } else {
                                        status_msg = format!("Remote statement executed successfully in {:.2?}.{}", elapsed, hint);
                                    }
                                } else if is_remote_db && is_unadorned_secret_or_setting(&query_to_execute) {
                                    status_msg = "⚠️ Applied LOCALLY on DuckPack! Use '-- @remote' at top of query to apply to remote Quack.".to_string();
                                } else {
                                    if is_catalog_mutation {
                                        trigger_explorer_refresh = true;
                                    }
                                    status_msg = format!("Query executed successfully. Returned {} rows in {:.2?}.", count, elapsed);
                                }

                                let history_path = queries_dir.join(".history.sql");
                                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                let history_entry = format!("-- Executed at {} (Took {:.2?})\n{};\n\n", timestamp, elapsed, query_to_execute.trim());
                                use std::io::Write;
                                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path) {
                                    let _ = f.write_all(history_entry.as_bytes());
                                }
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                                let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                                status_msg = format!("Execution Error: {}{}", e, diag);
                            }
                        }
                    }
                    Err(e) => {
                        let mut executed_remote_fallback = false;
                        let local_err_str = e.to_string();
                        let is_syntax_error = local_err_str.contains("syntax error at or near");
                        let lower_q = query_to_execute.to_lowercase();
                        let is_temp_mutation = lower_q.contains("temp table")
                            || lower_q.contains("temporary table")
                            || lower_q.contains("temp view")
                            || lower_q.contains("temporary view");
                        if is_quack && !is_explicit_remote && !is_syntax_error && !is_temp_mutation {
                            let clean_query = query_to_execute.trim().strip_suffix(';').unwrap_or(query_to_execute.trim());
                            let remote_sql = if is_paginatable {
                                format!("SELECT * FROM target_db.query('{}') LIMIT {} OFFSET {}", clean_query.replace("'", "''"), active_tab.pagination_limit, active_tab.pagination_offset)
                            } else {
                                format!("FROM target_db.query('{}');", clean_query.replace("'", "''"))
                            };
                            match conn.prepare(&remote_sql) {
                                Ok(mut remote_stmt) => {
                                    let start_time = std::time::Instant::now();
                                    match remote_stmt.query([]) {
                                        Ok(mut rows) => {
                                            let mut cols = Vec::new();
                                            if let Some(stmt_ref) = rows.as_ref() {
                                                for col in stmt_ref.column_names() {
                                                    cols.push(col.to_string());
                                                }
                                            }
                                            active_tab.column_names = cols;
                                            let mut exec_rows = Vec::new();
                                            let mut count = 0;
                                            while let Ok(Some(row)) = rows.next() {
                                                count += 1;
                                                let mut str_row = Vec::new();
                                                for i in 0..active_tab.column_names.len() {
                                                    let val: String = match row.get(i) {
                                                        Ok(v) => format_duckdb_value(v),
                                                        Err(_) => "NULL".to_string(),
                                                    };
                                                    str_row.push(val);
                                                }
                                                exec_rows.push(str_row);
                                            }
                                            active_tab.query_results = exec_rows;
                                            active_tab.results_state.select(Some(0));
                                            active_tab.horizontal_scroll = 0;
                                            let elapsed = start_time.elapsed();

                                            let lower = query_to_execute.to_lowercase();
                                            let is_catalog_mutation = lower.contains("attach ")
                                                || lower.contains("detach ")
                                                || lower.contains("create ")
                                                || lower.contains("drop ")
                                                || lower.contains("alter ")
                                                || lower.contains("use ");
                                            if is_catalog_mutation {
                                                refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                                trigger_explorer_refresh = true;
                                            }
                                            if count > 0 {
                                                status_msg = format!("Remote statement executed successfully. Returned {} rows in {:.2?}.", count, elapsed);
                                            } else {
                                                status_msg = format!("Remote statement executed successfully in {:.2?}.", elapsed);
                                            }
                                            executed_remote_fallback = true;

                                            let history_path = queries_dir.join(".history.sql");
                                            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                            let history_entry = format!("-- Executed at {} (Took {:.2?})\n{};\n\n", timestamp, elapsed, query_to_execute.trim());
                                            use std::io::Write;
                                            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path) {
                                                let _ = f.write_all(history_entry.as_bytes());
                                            }
                                        }
                                        Err(query_err) => {
                                            let err_str = query_err.to_string();
                                            let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                                            status_msg = format!("Execution Error: {}{}", query_err, diag);
                                            executed_remote_fallback = true;
                                        }
                                    }
                                }
                                Err(prep_err) => {
                                    let err_str = prep_err.to_string();
                                    let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                                    status_msg = format!("Prepare Error: {}{}", prep_err, diag);
                                    executed_remote_fallback = true;
                                }
                            }
                        }

                        if !executed_remote_fallback {
                            let err_str = e.to_string();
                            handle_sql_error_position(&mut tabs[active_tab_index].textarea, &err_str);
                            let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                            status_msg = format!("Prepare Error: {}{}", e, diag);
                        }
                    }
                }
                }
            }
        }

        if trigger_explorer_refresh {
            let prev_selected = explorer_state.selected();
            if explorer_mode == ExplorerMode::Project {
                let (b, st) = get_git_branch_and_status(project_dir);
                git_branch = b;
                git_status = st;
                explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
            } else {
                meta = load_explorer_metadata(conn, project_dir, is_quack);
                dictionary = meta.dictionary.clone();
                autocomplete_matches.clear();
                if !meta.queries.is_empty() {
                    expanded_groups.insert("Queries".to_string());
                }
                if meta.databases.len() > 1 {
                    for db in &meta.databases {
                        if db.is_active {
                            expanded_groups.insert(format!("db:{}", db.name));
                            expanded_groups.insert(format!("{}:Tables", db.name));
                        }
                    }
                } else {
                    expanded_groups.insert("Tables".to_string());
                }
                explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
            }
            if let Some(sel) = prev_selected {
                explorer_state.select(Some(sel.min(explorer_items.len().saturating_sub(1))));
            } else if !explorer_items.is_empty() {
                explorer_state.select(Some(0));
            }
        }

        if trigger_explorer_action {
            if let Some(selected) = explorer_state.selected() {
                if selected < explorer_items.len() {
                    let item_type = explorer_items[selected].node_type.clone();
                    let item_name = explorer_items[selected].name.clone();

                    match item_type {
                        NodeType::Database { .. } => {
                            let clean_db_name = if let Some(stripped) = item_name.strip_prefix("db:") {
                                stripped.to_string()
                            } else {
                                item_name.clone()
                            };
                            tabs[active_tab_index].target_database = clean_db_name.clone();
                            let _ = conn.execute(&format!("USE \"{}\";", clean_db_name.replace('"', "\"\"")), []);
                            meta.active_database = clean_db_name.clone();
                            for db in &mut meta.databases {
                                db.is_active = db.name == clean_db_name;
                            }
                            status_msg = format!("Target database switched to '{}' for active tab.", clean_db_name);

                            if expanded_groups.contains(&item_name) {
                                expanded_groups.remove(&item_name);
                            } else {
                                expanded_groups.insert(item_name);
                            }
                            explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                            explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::Group => {
                            if explorer_mode == ExplorerMode::Project {
                                if item_name.ends_with(":empty") {
                                    new_asset_modal.reset();
                                    if item_name.contains("tables") { new_asset_modal.category_index = 0; }
                                    else if item_name.contains("views") { new_asset_modal.category_index = 1; }
                                    else if item_name.contains("macros") { new_asset_modal.category_index = 2; }
                                    else if item_name.contains("triggers") { new_asset_modal.category_index = 3; }
                                    else if item_name.contains("schemas") { new_asset_modal.category_index = 4; }
                                    else if item_name.contains("queries") { new_asset_modal.category_index = 5; }
                                    else if item_name.contains("seeds") { new_asset_modal.category_index = 6; }
                                } else {
                                    if expanded_project_groups.contains(&item_name) {
                                        expanded_project_groups.remove(&item_name);
                                    } else {
                                        expanded_project_groups.insert(item_name);
                                    }
                                    explorer_items = rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
                                    explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                                }
                            } else {
                                if item_name.contains(":error:") {
                                    if let Some((_, err_text)) = item_name.split_once(":error:") {
                                        status_msg = format!("⚠️ Cloud Bucket Error: {}", err_text);
                                    }
                                } else if item_name.ends_with(":empty") {
                                    status_msg = "Bucket or directory is empty (0 matching objects found).".to_string();
                                } else {
                                    if expanded_groups.contains(&item_name) {
                                        expanded_groups.remove(&item_name);
                                    } else {
                                        expanded_groups.insert(item_name);
                                    }
                                    explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                                    explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                                }
                            }
                        }
                        NodeType::Table { database, schema, is_temporary: _ } | NodeType::View { database, schema, is_temporary: _ } => {
                            let key = match (&database, &schema) {
                                (Some(db), Some(sch)) if meta.databases.len() > 1 => format!("{}.{}.{}", db, sch, item_name),
                                _ => item_name.clone(),
                            };
                            if expanded_tables.contains(&key) || expanded_tables.contains(&item_name) {
                                expanded_tables.remove(&key);
                                expanded_tables.remove(&item_name);
                            } else {
                                expanded_tables.insert(key);
                            }
                            explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                            explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::Query => {
                            let path = queries_dir.join(&item_name);
                            
                            let mut already_open = None;
                            for (i, tab) in tabs.iter().enumerate() {
                                if tab.active_file.as_ref() == Some(&path) {
                                    already_open = Some(i);
                                    break;
                                }
                            }
                            
                            if let Some(i) = already_open {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                status_msg = format!("Switched to {}", item_name);
                            } else if let Ok(content) = std::fs::read_to_string(&path) {
                                let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                if !current_is_empty {
                                    tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                    active_tab_index = tabs.len() - 1;
                                }
                                
                                tabs[active_tab_index].textarea = TextArea::from(content.lines().map(|s| s.to_string()).collect::<Vec<_>>());
                                tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                tabs[active_tab_index].active_file = Some(path);
                                focus = FocusPane::Editor;
                                status_msg = format!("Loaded {}", item_name);
                            }
                        }
                        NodeType::ProjectFile { path, .. } => {
                            let mut already_open = None;
                            for (i, tab) in tabs.iter().enumerate() {
                                if tab.active_file.as_ref() == Some(&path) {
                                    already_open = Some(i);
                                    break;
                                }
                            }

                            if let Some(i) = already_open {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                status_msg = format!("Switched to {}", path.file_name().unwrap_or_default().to_string_lossy());
                            } else if let Ok(content) = std::fs::read_to_string(&path) {
                                let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                if !current_is_empty {
                                    tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                    active_tab_index = tabs.len() - 1;
                                }

                                tabs[active_tab_index].textarea = TextArea::from(content.lines().map(|s| s.to_string()).collect::<Vec<_>>());
                                tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                tabs[active_tab_index].active_file = Some(path.clone());
                                tabs[active_tab_index].is_dirty = false;
                                focus = FocusPane::Editor;
                                status_msg = format!("Loaded {}", path.file_name().unwrap_or_default().to_string_lossy());
                            }
                        }
                        NodeType::Column { .. } => {}
                        NodeType::Macro => {}
                        NodeType::Trigger { .. } => {}
                        NodeType::AddSecretAction => {
                            secret_modal.reset();
                        }
                        NodeType::AddBucketAction => {
                            add_bucket_modal.reset();
                        }
                        NodeType::CloudSecret { name, .. } => {
                            let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                            if !current_is_empty {
                                tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                active_tab_index = tabs.len() - 1;
                            }
                            let query = format!("SELECT * FROM duckdb_secrets() WHERE name = '{}';", name);
                            tabs[active_tab_index].textarea = TextArea::from(vec![query]);
                            tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                            focus = FocusPane::Editor;
                            status_msg = format!("Inspecting secret '{}'", name);
                        }
                        NodeType::CloudBucket { bucket } => {
                            let key = format!("bucket:{}", bucket);
                            if expanded_groups.contains(&key) {
                                expanded_groups.remove(&key);
                            } else {
                                expanded_groups.insert(key);
                            }
                            explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                            explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::CloudFile { path, is_dir } => {
                            if is_dir {
                                let key = format!("bucket_dir:{}", path);
                                if expanded_groups.contains(&key) {
                                    expanded_groups.remove(&key);
                                } else {
                                    expanded_groups.insert(key);
                                }
                                explorer_items = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);
                                explorer_state.select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                            } else {
                                let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
                                let read_fn = match ext.as_str() {
                                    "parquet" => "read_parquet",
                                    "csv" => "read_csv",
                                    "json" | "jsonl" | "ndjson" => "read_json",
                                    _ => "read_parquet",
                                };
                                let query = format!("SELECT * FROM {}('{}') LIMIT 500;", read_fn, path);
                                let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                if !current_is_empty {
                                    tabs.push(EditorTab::new(tabs[active_tab_index].target_database.clone()));
                                    active_tab_index = tabs.len() - 1;
                                }
                                tabs[active_tab_index].textarea = TextArea::from(vec![query]);
                                tabs[active_tab_index].textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
                                tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                focus = FocusPane::Editor;
                                let fname = path.split('/').last().unwrap_or(&path);
                                status_msg = format!("Queried cloud file '{}'", fname);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Extracts error location (line, column) and token from DuckDB v2.0 PEG parser errors
/// or legacy Postgres parser errors, jumping the cursor and styling the error token in bold red.
pub fn handle_sql_error_position(textarea: &mut TextArea, err_str: &str) {
    let lower = err_str.to_lowercase();
    let mut found_pos: Option<(usize, usize)> = None;

    // Pattern 1: DuckDB PEG parser "line (\d+), column (\d+)" or "syntax error at line (\d+), column (\d+)"
    if let Some(line_idx) = lower.find("line ") {
        let rest = &lower[line_idx + 5..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(line_num) = num_str.parse::<usize>() {
            let after_line = &rest[num_str.len()..];
            let mut col_num = 1;
            if let Some(col_idx) = after_line.find("col") {
                let col_rest = &after_line[col_idx + 3..];
                let col_digits = col_rest.trim_start_matches("umn").trim_start_matches(|c: char| c.is_whitespace() || c == ':');
                let col_num_str: String = col_digits.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(parsed_col) = col_num_str.parse::<usize>() {
                    col_num = parsed_col;
                }
            }
            found_pos = Some((line_num, col_num));
        }
    }

    // Jump cursor if position was parsed
    if let Some((line, col)) = found_pos {
        let row_idx = (line.saturating_sub(1)) as u16;
        let col_idx = (col.saturating_sub(1)) as u16;
        textarea.move_cursor(CursorMove::Jump(row_idx, col_idx));
    }

    // Highlight error token:
    // Case A: legacy DuckDB 1.x "syntax error at or near \"<word>\""
    if err_str.contains("syntax error at or near") {
        if let Some(pos) = err_str.find('"') {
            if let Some(end_pos) = err_str[pos + 1..].find('"') {
                let broken_word = &err_str[pos + 1..pos + 1 + end_pos];
                textarea.set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(broken_word))).unwrap_or(());
                textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                return;
            }
        }
    }

    // Case B: token under cursor if line and col found
    if let Some((line, col)) = found_pos {
        let lines = textarea.lines();
        let row_idx = line.saturating_sub(1);
        if row_idx < lines.len() {
            let line_text = &lines[row_idx];
            let col_idx = col.saturating_sub(1);
            if col_idx < line_text.len() {
                let tail = &line_text[col_idx..];
                let token: String = tail.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !token.is_empty() {
                    textarea.set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(&token))).unwrap_or(());
                    textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                }
            }
        }
    }
}

/// Translates native DuckDB data types into readable strings for the IDE Results pane.
/// Provides precise formatting for Dates, Timestamps, Numerics, and Arrays/Lists.
fn format_duckdb_value(v: duckdb::types::Value) -> String {
    use duckdb::types::{Value, TimeUnit};
    match v {
        Value::Null => "NULL".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(v) => v.to_string(),
        Value::SmallInt(v) => v.to_string(),
        Value::Int(v) => v.to_string(),
        Value::BigInt(v) => v.to_string(),
        Value::HugeInt(v) => v.to_string(),
        Value::UTinyInt(v) => v.to_string(),
        Value::USmallInt(v) => v.to_string(),
        Value::UInt(v) => v.to_string(),
        Value::UBigInt(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Double(v) => v.to_string(),
        Value::Text(t) => t,
        Value::Blob(_) => "[BLOB]".to_string(),
        Value::Decimal(d) => {
            let s = format!("{:?}", d);
            if s.starts_with("Decimal(") && s.ends_with(")") {
                s[8..s.len()-1].to_string()
            } else {
                s
            }
        },
        Value::Timestamp(unit, val) => {
            let timestamp_secs = match unit {
                TimeUnit::Second => val,
                TimeUnit::Millisecond => val / 1000,
                TimeUnit::Microsecond => val / 1_000_000,
                TimeUnit::Nanosecond => val / 1_000_000_000,
            };
            let timestamp_nanos = match unit {
                TimeUnit::Second => 0,
                TimeUnit::Millisecond => (val % 1000) * 1_000_000,
                TimeUnit::Microsecond => (val % 1_000_000) * 1000,
                TimeUnit::Nanosecond => val % 1_000_000_000,
            };
            if let Some(dt) = chrono::DateTime::from_timestamp(timestamp_secs, timestamp_nanos as u32) {
                dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
            } else {
                format!("{:?}", Value::Timestamp(unit, val))
            }
        }
        Value::Date32(days) => {
            if let Some(dt) = chrono::DateTime::from_timestamp((days as i64) * 86400, 0) {
                dt.naive_utc().date().format("%Y-%m-%d").to_string()
            } else {
                format!("{:?}", Value::Date32(days))
            }
        }
        Value::Time64(unit, val) => {
            let timestamp_nanos = match unit {
                TimeUnit::Second => val * 1_000_000_000,
                TimeUnit::Millisecond => val * 1_000_000,
                TimeUnit::Microsecond => val * 1000,
                TimeUnit::Nanosecond => val,
            };
            let secs = (timestamp_nanos / 1_000_000_000) as u32;
            let nanos = (timestamp_nanos % 1_000_000_000) as u32;
            if let Some(t) = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, nanos) {
                t.format("%H:%M:%S%.6f").to_string()
            } else {
                format!("{:?}", Value::Time64(unit, val))
            }
        }
        _ => {
            let s = format!("{:?}", v);
            if let Some(stripped) = s.split('(').next() {
                if s.ends_with(')') && s.len() > stripped.len() {
                    return s[stripped.len()+1..s.len()-1].to_string();
                }
            }
            s
        }
    }
}

/// Generates an ASCII tree of the schema and copies a Mermaid diagram to the clipboard.
fn generate_schema_visualizer(conn: &duckdb::Connection, os_clipboard: &mut Option<arboard::Clipboard>, is_quack: bool) -> Result<String, Box<dyn std::error::Error>> {
    let mut remote_dbs: Vec<(String, String)> = Vec::new();
    if is_quack {
        let quack_db_sql = "SELECT database_name, type FROM target_db.query('SELECT database_name, type FROM duckdb_databases() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_db_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        remote_dbs.push((name, typ));
                    }
                }
            }
        }
    }
    let is_remote_multi_db = remote_dbs.len() > 1;

    let mut db_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut tables: std::collections::BTreeMap<(String, String, String), Vec<(String, String)>> = std::collections::BTreeMap::new();

    if is_remote_multi_db {
        let col_query = "SELECT database_name, schema_name, table_name, column_name, data_type FROM target_db.query('SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY database_name, schema_name, table_name, column_index');";
        if let Ok(mut stmt) = conn.prepare(col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3), row.get::<_, String>(4)) {
                        db_set.insert(db_name.clone());
                        tables.entry((db_name, schema_name, table_name)).or_default().push((col_name, data_type));
                    }
                }
            }
        }
    } else {
        // Query local columns on conn
        let col_query = "SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY database_name, schema_name, table_name, column_index;";
        if let Ok(mut stmt) = conn.prepare(col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3), row.get::<_, String>(4)) {
                        db_set.insert(db_name.clone());
                        tables.entry((db_name, schema_name, table_name)).or_default().push((col_name, data_type));
                    }
                }
            }
        }
        // If single remote quack is attached, query target_db columns directly
        if is_quack {
            let quack_cols = "SELECT 'target_db' AS database_name, schema_name, table_name, column_name, data_type FROM target_db.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY schema_name, table_name, column_index')";
            if let Ok(mut stmt) = conn.prepare(quack_cols) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2), row.get::<_, String>(3), row.get::<_, String>(4)) {
                            db_set.insert(db_name.clone());
                            tables.entry((db_name, schema_name, table_name)).or_default().push((col_name, data_type));
                        }
                    }
                }
            }
        }
    }

    // Also get columns from attached quack databases
    let dbs_query = if is_remote_multi_db {
        "SELECT database_name, type FROM target_db.query('SELECT database_name, type FROM duckdb_databases() WHERE internal = false')".to_string()
    } else {
        "SELECT database_name, type FROM duckdb_databases() WHERE internal = false".to_string()
    };
    if let Ok(mut d_stmt) = conn.prepare(&dbs_query) {
        if let Ok(mut d_rows) = d_stmt.query([]) {
            while let Ok(Some(row)) = d_rows.next() {
                if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    if typ.eq_ignore_ascii_case("quack") {
                        if !is_remote_multi_db && name == "target_db" {
                            // Already queried above
                            continue;
                        }
                        let safe_name = name.replace("'", "''");
                        let q = if is_remote_multi_db {
                            format!(
                                "SELECT * FROM target_db.query('SELECT ''{}'' AS database_name, schema_name, table_name, column_name, data_type FROM {}.query(''SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY table_name, column_index'')')",
                                safe_name, safe_name
                            )
                        } else {
                            format!(
                                "SELECT '{}' AS database_name, schema_name, table_name, column_name, data_type FROM {}.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY table_name, column_index')",
                                safe_name, safe_name
                            )
                        };
                        if let Ok(mut q_stmt) = conn.prepare(&q) {
                            if let Ok(mut q_rows) = q_stmt.query([]) {
                                while let Ok(Some(q_row)) = q_rows.next() {
                                    if let (Ok(db), Ok(schema), Ok(table), Ok(col), Ok(dtype)) = (q_row.get(0), q_row.get(1), q_row.get(2), q_row.get(3), q_row.get(4)) {
                                        db_set.insert(db);
                                        tables.entry((name.clone(), schema, table)).or_default().push((col, dtype));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // 2. Get Foreign Keys
    let mut fks: Vec<(String, String, String, String)> = Vec::new();
    if let Ok(mut stmt_fk) = conn.prepare("SELECT table_name, constraint_column_names[1], referenced_table, referenced_column_names[1] FROM duckdb_constraints() WHERE constraint_type = 'FOREIGN KEY';") {
        if let Ok(mut rows_fk) = stmt_fk.query([]) {
            while let Ok(Some(row)) = rows_fk.next() {
                if let (Ok(table_name), Ok(col_name), Ok(ref_table), Ok(ref_col)) = (row.get(0), row.get(1), row.get(2), row.get(3)) {
                    fks.push((table_name, col_name, ref_table, ref_col));
                }
            }
        }
    }
    
    // 3. Generate Mermaid
    let mut mermaid = String::from("erDiagram\n");
    for ((db, _schema, table), cols) in &tables {
        let entity_name = if db_set.len() > 1 {
            format!("{}_{}", db.replace(|c: char| !c.is_alphanumeric(), "_"), table.replace(|c: char| !c.is_alphanumeric(), "_"))
        } else {
            table.replace(|c: char| !c.is_alphanumeric(), "_")
        };
        mermaid.push_str(&format!("    {} {{\n", entity_name));
        for (col, dtype) in cols {
            mermaid.push_str(&format!("        {} {}\n", dtype.replace(" ", "_"), col.replace(|c: char| !c.is_alphanumeric(), "_")));
        }
        mermaid.push_str("    }\n");
    }
    for (table, _col, ref_table, _ref_col) in &fks {
        let t1 = table.replace(|c: char| !c.is_alphanumeric(), "_");
        let t2 = ref_table.replace(|c: char| !c.is_alphanumeric(), "_");
        mermaid.push_str(&format!("    {} }}|--|| {} : \"\"\n", t1, t2));
    }
    
    // Copy Mermaid diagram using multi-channel clipboard helper
    set_clipboard_text(&mermaid, os_clipboard);
    
    // 4. Generate ASCII Tree
    let mut tree = String::from("Database Schema (Mermaid.js diagram copied to clipboard!)\n");
    if db_set.len() > 1 {
        let db_list: Vec<&String> = db_set.iter().collect();
        for (i, &db) in db_list.iter().enumerate() {
            let is_last_db = i == db_list.len() - 1;
            let db_prefix = if is_last_db { "└──" } else { "├──" };
            tree.push_str(&format!("{} 📁 {}\n", db_prefix, db));
            
            let db_tables: Vec<(&(String, String, String), &Vec<(String, String)>)> = tables.iter().filter(|((d, _, _), _)| d == db).collect();
            for (j, ((_, schema, table), cols)) in db_tables.iter().enumerate() {
                let is_last_table = j == db_tables.len() - 1;
                let table_prefix = if is_last_db { "    " } else { "│   " };
                let branch = if is_last_table { "└──" } else { "├──" };
                let display_tbl = if schema == "main" { table.clone() } else { format!("{}.{}", schema, table) };
                tree.push_str(&format!("{}{} {}\n", table_prefix, branch, display_tbl));
                
                for (k, (col, dtype)) in cols.iter().enumerate() {
                    let is_last_col = k == cols.len() - 1;
                    let col_prefix = if is_last_db {
                        if is_last_table { "        " } else { "    │   " }
                    } else {
                        if is_last_table { "│       " } else { "│   │   " }
                    };
                    let col_branch = if is_last_col { "└──" } else { "├──" };
                    let fk = fks.iter().find(|(t, c, _, _)| t == table && c == col);
                    if let Some((_, _, ref_table, ref_col)) = fk {
                        tree.push_str(&format!("{}{}{} ({}) -> {}.{}\n", col_prefix, col_branch, col, dtype, ref_table, ref_col));
                    } else {
                        tree.push_str(&format!("{}{}{} ({})\n", col_prefix, col_branch, col, dtype));
                    }
                }
            }
        }
    } else {
        let tbl_list: Vec<(&(String, String, String), &Vec<(String, String)>)> = tables.iter().collect();
        for (i, ((_, _schema, table), cols)) in tbl_list.iter().enumerate() {
            let is_last_table = i == tbl_list.len() - 1;
            let prefix = if is_last_table { "└──" } else { "├──" };
            tree.push_str(&format!("{} {}\n", prefix, table));
            
            for (j, (col, dtype)) in cols.iter().enumerate() {
                let is_last_col = j == cols.len() - 1;
                let col_prefix = if is_last_table { "    " } else { "│   " };
                let line_prefix = if is_last_col { "└──" } else { "├──" };
                
                let fk = fks.iter().find(|(t, c, _, _)| t == table && c == col);
                if let Some((_, _, ref_table, ref_col)) = fk {
                    tree.push_str(&format!("{}{} {} ({}) -> {}.{}\n", col_prefix, line_prefix, col, dtype, ref_table, ref_col));
                } else {
                    tree.push_str(&format!("{}{} {} ({})\n", col_prefix, line_prefix, col, dtype));
                }
            }
        }
    }
    
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::types::Value;

    #[test]
    fn test_format_duckdb_value() {
        assert_eq!(format_duckdb_value(Value::Null), "NULL");
        assert_eq!(format_duckdb_value(Value::Boolean(true)), "true");
        assert_eq!(format_duckdb_value(Value::Int(42)), "42");
        assert_eq!(format_duckdb_value(Value::Text("hello".to_string())), "hello");
    }

    #[test]
    fn test_wide_table_column_windowing() {
        let cols: Vec<String> = (0..217).map(|i| format!("col_{}", i)).collect();
        let rows: Vec<Vec<String>> = vec![
            (0..217).map(|i| format!("val_{}", i)).collect(),
        ];

        let avail_width: usize = 80;
        let mut visible = Vec::new();
        let mut used = 0;
        for i in 0..cols.len() {
            let col_w = (cols[i].len().max(rows[0][i].len()) + 2).clamp(6, 30);
            if visible.is_empty() || used + col_w <= avail_width {
                used += col_w;
                visible.push(i);
            } else {
                break;
            }
        }
        assert!(!visible.is_empty());
        assert!(visible.len() < cols.len());
        assert_eq!(visible[0], 0);
        assert!(used <= avail_width);
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
    fn test_calculate_status_height() {
        // Single line short status
        assert_eq!(calculate_status_height("Ready.", 80, 30), 3);

        // Long status that wraps to 2 lines
        let long_msg = "A".repeat(100);
        assert_eq!(calculate_status_height(&long_msg, 80, 30), 4);

        // Multi-line status with newlines
        let multi_line = "Error: Line 1\nLine 2\nLine 3";
        assert_eq!(calculate_status_height(multi_line, 80, 30), 5);

        // Respects maximum limit clamped to total_height / 3 or 8
        let huge_msg = "Error:\n".repeat(20);
        assert_eq!(calculate_status_height(&huge_msg, 80, 30), 8);
    }

    #[test]
    fn test_clipboard_roundtrip() {
        let mut cb: Option<arboard::Clipboard> = None;
        let test_str = "duckpack_test_clipboard_12345";
        assert!(set_clipboard_text(test_str, &mut cb));
        let retrieved = get_clipboard_text(&mut cb);
        if let Some(ref text) = retrieved {
            assert_eq!(text, test_str);
        }
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

        // Rebuild single DB nodes: flat layout
        let mut expanded_groups = std::collections::HashSet::new();
        let mut expanded_tables = std::collections::HashSet::new();
        expanded_groups.insert("Tables".to_string());
        let nodes_single = rebuild_explorer_nodes(&meta_single, &expanded_groups, &expanded_tables, &conn, false);
        assert!(nodes_single.iter().any(|n| n.display.contains("Tables (1)")));
        assert!(nodes_single.iter().any(|n| n.name == "users"));

        // 2. Attach a second database
        let attached_db_path = temp_dir.join("analytics.duckdb");
        conn.execute(&format!("ATTACH '{}' AS analytics;", attached_db_path.display()), []).unwrap();
        conn.execute("CREATE TABLE analytics.events (event_id BIGINT, event_name VARCHAR);", []).unwrap();

        let meta_multi = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta_multi.databases.len(), 2);
        assert!(meta_multi.databases.iter().any(|d| d.name == "memory" && d.is_active));
        assert!(meta_multi.databases.iter().any(|d| d.name == "analytics" && !d.is_active));

        // Rebuild multi-DB nodes: catalog-aware hierarchy
        expanded_groups.clear();
        expanded_tables.clear();
        expanded_groups.insert("db:analytics".to_string());
        expanded_groups.insert("analytics:Tables".to_string());
        expanded_tables.insert("analytics.main.events".to_string());

        let nodes_multi = rebuild_explorer_nodes(&meta_multi, &expanded_groups, &expanded_tables, &conn, false);
        assert!(nodes_multi.iter().any(|n| n.name == "db:analytics" && n.display.contains("analytics")));
        assert!(nodes_multi.iter().any(|n| n.name == "analytics:Tables"));
        assert!(nodes_multi.iter().any(|n| n.name == "events"));
        assert!(nodes_multi.iter().any(|n| n.name == "event_id"));

        // 3. Schema visualizer with attached database
        let mut cb = None;
        let visualizer_result = generate_schema_visualizer(&conn, &mut cb, false);
        assert!(visualizer_result.is_ok());
        let tree_text = visualizer_result.unwrap();
        assert!(tree_text.contains("📁 memory"));
        assert!(tree_text.contains("📁 analytics"));
        assert!(tree_text.contains("events"));

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
    fn test_dot_autocomplete_word_extraction() {
        // Multi-part dot-qualified identifiers extraction
        let line = "SELECT analytics.main.events.event_id FROM t";
        let cursor_char_idx = 37; // right at the end of "event_id" (len of "SELECT analytics.main.events.event_id")
        let prefix: String = line.chars().take(cursor_char_idx).collect();
        let word_start = prefix.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.').map(|i| i + 1).unwrap_or(0);
        let current_word = &prefix[word_start..];
        assert_eq!(current_word, "analytics.main.events.event_id");

        let line2 = "SELECT db.";
        let prefix2: String = line2.chars().collect();
        let word_start2 = prefix2.rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.').map(|i| i + 1).unwrap_or(0);
        let current_word2 = &prefix2[word_start2..];
        assert_eq!(current_word2, "db.");
    }

    #[test]
    fn test_live_quack_metadata_loading() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        // Connect to live quack container if running
        let conn_res = crate::engine::connect_target_db(db_uri, Some(&token_var), true);
        if let Ok(conn) = conn_res {
            let temp_dir = std::env::temp_dir().join(format!("dp_live_quack_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            let _ = std::fs::create_dir_all(&temp_dir);

            conn.execute("CREATE TEMP TABLE t_test AS SELECT 1;", []).unwrap();
            let meta = load_explorer_metadata(&conn, &temp_dir, true);
            println!("Databases found: {:?}", meta.databases.iter().map(|d| (&d.name, &d.db_type, d.is_active, d.tables.len())).collect::<Vec<_>>());
            assert_eq!(meta.databases.len(), 2, "Expected target_db and memory");
            assert!(meta.databases.iter().any(|d| d.name == "target_db" && d.is_active));
            assert!(meta.databases.iter().any(|d| d.name == "memory" && !d.is_active));

            let target_db = meta.databases.iter().find(|d| d.name == "target_db").unwrap();
            assert!(target_db.tables.iter().any(|t| t.name == "test_tbl"), "Expected test_tbl in target_db");

            let mut expanded_groups = std::collections::HashSet::new();
            let expanded_tables = std::collections::HashSet::new();
            expanded_groups.insert("db:target_db".to_string());
            expanded_groups.insert("target_db:Tables".to_string());
            expanded_groups.insert("db:memory".to_string());

            let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let node_displays: Vec<String> = nodes.iter().map(|n| n.display.clone()).collect();
            println!("Nodes rendered:\n{}", node_displays.join("\n"));

            assert!(nodes.iter().any(|n| n.name == "db:target_db" && n.display.contains("target_db")));
            assert!(nodes.iter().any(|n| n.name == "db:memory" && n.display.contains("memory")));
            assert!(nodes.iter().any(|n| n.name == "test_tbl"));

            let cols = load_table_columns_for("target_db", "main", "test_tbl", &conn, true);
            println!("Columns for test_tbl: {:?}", cols);
            assert!(cols.iter().any(|c| c.name == "column_a"));

            // Verify PK detection on live Quack for products table
            let prod_cols = load_table_columns_for("target_db", "main", "products", &conn, true);
            let pk_col = prod_cols.iter().find(|c| c.name == "product_id").expect("product_id not found in products");
            assert!(pk_col.is_pk, "product_id should be identified as primary key");

            let mut expanded_tables = std::collections::HashSet::new();
            expanded_tables.insert("products".to_string());
            let nodes_with_cols = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let pk_node = nodes_with_cols.iter().find(|n| n.name == "product_id").expect("product_id node not found");
            assert!(pk_node.display.contains("🔑 product_id") && pk_node.display.contains("[PK]"), "Expected key icon and [PK] tag, got: {}", pk_node.display);
            println!("Verified live Quack Explorer PK rendering: {}", pk_node.display);

            // Verify FK detection and rendering on live Quack for orders table
            let order_cols = load_table_columns_for("target_db", "main", "orders", &conn, true);
            let user_id_col = order_cols.iter().find(|c| c.name == "user_id").expect("user_id not found in orders");
            assert!(user_id_col.is_fk, "user_id should be identified as foreign key");
            assert_eq!(user_id_col.fk_target.as_deref(), Some("users.user_id"));

            expanded_tables.insert("orders".to_string());
            let nodes_with_orders = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let fk_node = nodes_with_orders.iter().find(|n| n.name == "user_id" && n.display.contains("[FK")).expect("user_id FK node not found");
            assert!(fk_node.display.contains("🔗 user_id") && fk_node.display.contains("[FK -> users.user_id]"), "Expected FK icon and target, got: {}", fk_node.display);
            println!("Verified live Quack Explorer FK rendering: {}", fk_node.display);

            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_live_insert_users() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
            let temp_dir = std::env::temp_dir().join(format!("dp_live_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            let _ = std::fs::create_dir_all(&temp_dir);

            let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
            let q = "insert into users values (9999, 'Test', 'User', '1900-01-01', 'test_user@example.com', now(), now(), True);";
            
            // Execute mutation via execute_batch
            let res = conn.execute_batch(q);
            assert!(res.is_ok(), "execute_batch failed: {:?}", res);

            let count: i64 = conn.query_row("SELECT count(*) FROM target_db.users WHERE user_id = 9999;", [], |r| r.get(0)).unwrap();
            assert_eq!(count, 1, "Expected exactly 1 row inserted, got {}", count);
            println!("Verified: Exactly 1 row inserted via execute_batch for users!");

            // Verify load_explorer_metadata returns the EXACT live count for users
            let meta = load_explorer_metadata(&conn, &temp_dir, true);
            let target_db = meta.databases.iter().find(|d| d.name == "target_db").unwrap();
            let users_table = target_db.tables.iter().find(|t| t.name == "users").unwrap();
            let actual_db_count: i64 = conn.query_row("SELECT count(*) FROM target_db.users;", [], |r| r.get(0)).unwrap();
            assert_eq!(users_table.row_count, Some(actual_db_count), "Explorer metadata row_count must match actual count(*)");
            println!("Verified: Explorer metadata row count {} matches actual DB count {}", users_table.row_count.unwrap(), actual_db_count);

            let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
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

        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Tables".to_string());
        expanded_groups.insert("Views".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let tbl_node = nodes.iter().find(|n| n.name == "staging_tbl").expect("staging_tbl node not found");
        assert!(tbl_node.display.contains("[temp]"), "Expected [temp] in display: {}", tbl_node.display);
        match &tbl_node.node_type {
            NodeType::Table { is_temporary, .. } => assert!(*is_temporary),
            _ => panic!("Expected NodeType::Table"),
        }

        let view_node = nodes.iter().find(|n| n.name == "staging_view").expect("staging_view node not found");
        assert!(view_node.display.contains("[temp]"), "Expected [temp] in display: {}", view_node.display);
        match &view_node.node_type {
            NodeType::View { is_temporary, .. } => assert!(*is_temporary),
            _ => panic!("Expected NodeType::View"),
        }

        let cols = load_table_columns_for(&meta.active_database, "main", "staging_tbl", &conn, false);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[1].name, "label");

        let view_cols = load_table_columns_for(&meta.active_database, "main", "staging_view", &conn, false);
        assert_eq!(view_cols.len(), 2);
        assert_eq!(view_cols[0].name, "id");
        assert_eq!(view_cols[1].name, "label");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_handle_sql_error_position_peg() {
        let mut textarea = TextArea::from(vec![
            "SELECT *".to_string(),
            "FROM non_existent".to_string(),
            "WHEREE x = 1".to_string(),
        ]);
        let err = "Parser Error: syntax error at line 3, column 1";
        handle_sql_error_position(&mut textarea, err);
        let (row, col) = textarea.cursor();
        assert_eq!(row, 2);
        assert_eq!(col, 0);
    }

    #[test]
    fn test_handle_sql_error_position_legacy() {
        let mut textarea = TextArea::from(vec![
            "SELCT 1;".to_string(),
        ]);
        let err = "Parser Error: syntax error at or near \"SELCT\"";
        handle_sql_error_position(&mut textarea, err);
    }

    #[test]
    fn test_triggers_in_explorer() {
        let temp_dir = std::env::temp_dir().join(format!("dp_trg_exp_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE orders (id INT, total DOUBLE);", []).unwrap();
        conn.execute("CREATE TABLE order_audit (id INT);", []).unwrap();
        conn.execute("CREATE TRIGGER trg_audit AFTER INSERT ON orders FOR EACH ROW INSERT INTO order_audit VALUES (NEW.id);", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta.triggers.len(), 1);
        assert!(meta.triggers[0].contains("trg_audit"));

        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Triggers".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let trg_node = nodes.iter().find(|n| n.name.contains("trg_audit")).expect("trg_audit node not found");
        assert!(trg_node.display.contains("trg_audit"));
        match &trg_node.node_type {
            NodeType::Trigger { .. } => {},
            _ => panic!("Expected NodeType::Trigger"),
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_explorer_fk_and_pk_column_rendering() {
        // Direct test of format_column_display helper
        assert_eq!(
            format_column_display("  ", "id", "INTEGER", true, false, None),
            "  └─ 🔑 id (INTEGER) [PK]"
        );
        assert_eq!(
            format_column_display("  ", "parent_id", "INTEGER", false, true, Some("parent.id")),
            "  └─ 🔗 parent_id (INTEGER) [FK -> parent.id]"
        );
        assert_eq!(
            format_column_display("  ", "parent_id", "INTEGER", true, true, Some("parent.id")),
            "  └─ 🔑🔗 parent_id (INTEGER) [PK, FK -> parent.id]"
        );
        assert_eq!(
            format_column_display("  ", "name", "VARCHAR", false, false, None),
            "  └─ name (VARCHAR)"
        );

        // Integration test with database catalog and rebuild_explorer_nodes
        let temp_dir = std::env::temp_dir().join(format!("dp_fk_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE parent (id INT PRIMARY KEY, name VARCHAR);", []).unwrap();
        conn.execute("CREATE TABLE child (child_id INT PRIMARY KEY, parent_id INT REFERENCES parent(id), data VARCHAR);", []).unwrap();
        conn.execute("CREATE TABLE composite_child (parent_id INT REFERENCES parent(id), sub_id INT, PRIMARY KEY (parent_id, sub_id));", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Tables".to_string());
        let mut expanded_tables = HashSet::new();
        expanded_tables.insert("child".to_string());
        expanded_tables.insert("composite_child".to_string());

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

        // Check child table columns
        let child_pk = nodes.iter().find(|n| n.name == "child_id").expect("child_id not found");
        assert!(child_pk.display.contains("🔑 child_id"), "Expected 🔑 for child_id: {}", child_pk.display);
        assert!(child_pk.display.contains("[PK]"), "Expected [PK] for child_id: {}", child_pk.display);
        assert!(!child_pk.display.contains("🔗"), "child_id should not have FK icon");

        let parent_id_nodes: Vec<_> = nodes.iter().filter(|n| n.name == "parent_id").collect();
        assert_eq!(parent_id_nodes.len(), 2, "Expected 2 parent_id columns (child and composite_child)");

        let child_fk = parent_id_nodes[0];
        assert!(child_fk.display.contains("🔗 parent_id"), "Expected 🔗 for parent_id in child: {}", child_fk.display);
        assert!(child_fk.display.contains("[FK -> parent.id]"), "Expected [FK -> parent.id]: {}", child_fk.display);
        assert!(!child_fk.display.contains("🔑"), "parent_id in child should not have PK icon");

        // Check composite PK+FK table
        let composite_fk = parent_id_nodes[1];
        assert!(composite_fk.display.contains("🔑🔗 parent_id"), "Expected 🔑🔗 for composite parent_id: {}", composite_fk.display);
        assert!(composite_fk.display.contains("[PK, FK -> parent.id]"), "Expected [PK, FK -> parent.id]: {}", composite_fk.display);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_new_asset_modal_state() {
        let mut modal = NewAssetModalState::new();
        assert!(!modal.is_open);
        assert_eq!(modal.category_index, 0);
        assert_eq!(modal.current_category(), ProjectCategory::Table);
        assert_eq!(modal.category_label(), "Table");

        modal.reset();
        assert!(modal.is_open);
        assert_eq!(modal.active_field, 1);
        assert_eq!(modal.name, "");

        modal.category_index = 1;
        assert_eq!(modal.current_category(), ProjectCategory::View);
        assert_eq!(modal.category_label(), "View");

        modal.category_index = 2;
        assert_eq!(modal.current_category(), ProjectCategory::Macro);
        assert_eq!(modal.category_label(), "Macro");

        modal.category_index = 3;
        assert_eq!(modal.current_category(), ProjectCategory::Trigger);
        assert_eq!(modal.category_label(), "Trigger");

        modal.category_index = 4;
        assert_eq!(modal.current_category(), ProjectCategory::Schema);
        assert_eq!(modal.category_label(), "Schema");

        modal.category_index = 5;
        assert_eq!(modal.current_category(), ProjectCategory::Query);
        assert_eq!(modal.category_label(), "Query");

        modal.category_index = 6;
        assert_eq!(modal.current_category(), ProjectCategory::Seed);
        assert_eq!(modal.category_label(), "Seed");
    }

    #[test]
    fn test_generate_asset_boilerplate() {
        let table_sql = generate_asset_boilerplate(ProjectCategory::Table, "customers");
        assert!(table_sql.contains("CREATE TABLE customers"));
        assert!(table_sql.contains("id INTEGER PRIMARY KEY"));

        let view_sql = generate_asset_boilerplate(ProjectCategory::View, "active_users.sql");
        assert!(view_sql.contains("CREATE VIEW active_users AS"));

        let macro_sql = generate_asset_boilerplate(ProjectCategory::Macro, "to_cents");
        assert!(macro_sql.contains("CREATE MACRO to_cents(val)"));

        let trigger_sql = generate_asset_boilerplate(ProjectCategory::Trigger, "trg_audit");
        assert!(trigger_sql.contains("CREATE OR REPLACE TRIGGER trg_audit"));
        assert!(trigger_sql.contains("FOR EACH ROW"));

        let schema_sql = generate_asset_boilerplate(ProjectCategory::Schema, "analytics");
        assert!(schema_sql.contains("CREATE SCHEMA IF NOT EXISTS analytics"));

        let query_sql = generate_asset_boilerplate(ProjectCategory::Query, "adhoc_report");
        assert!(query_sql.contains("SELECT 1;"));

        let seed_data = generate_asset_boilerplate(ProjectCategory::Seed, "sample_data");
        assert!(seed_data.contains("id,name,value"));
    }

    #[test]
    fn test_get_asset_file_path() {
        let root = PathBuf::from("/test/project");
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Schema, "staging"),
            root.join("schemas").join("staging.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Table, "orders.sql"),
            root.join("tables").join("orders.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::View, "v_orders"),
            root.join("views").join("v_orders.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Macro, "my_macro"),
            root.join("macros").join("my_macro.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Trigger, "orders_audit"),
            root.join("triggers").join("orders_audit.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Query, "analysis"),
            root.join("queries").join("analysis.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Seed, "countries"),
            root.join("seeds").join("countries.csv")
        );
    }

    #[test]
    fn test_rebuild_project_nodes_and_git_badges() {
        let temp_dir = std::env::temp_dir().join(format!("dp_proj_nodes_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();
        std::fs::create_dir_all(temp_dir.join("views")).unwrap();
        std::fs::create_dir_all(temp_dir.join("macros")).unwrap();

        let table_file = temp_dir.join("tables").join("users.sql");
        std::fs::write(&table_file, "CREATE TABLE users (id INT PRIMARY KEY);").unwrap();

        let view_file = temp_dir.join("views").join("v_users.sql");
        std::fs::write(&view_file, "CREATE VIEW v_users AS SELECT * FROM users;").unwrap();

        std::fs::write(temp_dir.join("duckpack.toml"), "[project]\nname = \"test\"\n").unwrap();

        let mut git_status = HashMap::new();
        git_status.insert(table_file.clone(), "[M]".to_string());
        git_status.insert(view_file.clone(), "[+]".to_string());

        let mut expanded = HashSet::new();
        expanded.insert("proj:tables".to_string());
        expanded.insert("proj:views".to_string());
        expanded.insert("proj:triggers".to_string()); // triggers directory doesn't exist on disk

        let nodes = rebuild_project_nodes(&temp_dir, &expanded, &git_status);

        // Verify tables group
        let table_group = nodes.iter().find(|n| n.name == "proj:tables").expect("proj:tables group not found");
        assert!(table_group.display.contains("Tables (1)"));

        // Verify users.sql with [M] badge
        let user_node = nodes.iter().find(|n| n.name == "users.sql").expect("users.sql node not found");
        assert!(user_node.display.contains("[M]"));

        // Verify views group
        let view_group = nodes.iter().find(|n| n.name == "proj:views").expect("proj:views group not found");
        assert!(view_group.display.contains("Views (1)"));

        // Verify v_users.sql with [+] badge
        let view_node = nodes.iter().find(|n| n.name == "v_users.sql").expect("v_users.sql node not found");
        assert!(view_node.display.contains("[+]"));

        // Verify triggers group is present even though triggers/ dir did not exist
        let trigger_group = nodes.iter().find(|n| n.name == "proj:triggers").expect("proj:triggers group not found");
        assert!(trigger_group.display.contains("Triggers (0)"));

        // Verify triggers empty placeholder node is present
        let empty_trigger_node = nodes.iter().find(|n| n.name == "proj:triggers:empty").expect("proj:triggers:empty placeholder not found");
        assert!(empty_trigger_node.display.contains("empty - press 'a' to add"));

        // Verify duckpack.toml config node
        let config_node = nodes.iter().find(|n| n.name == "duckpack.toml").expect("duckpack.toml node not found");
        assert!(config_node.display.contains("duckpack.toml"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compute_and_apply_ide_diff() {
        let temp_dir = std::env::temp_dir().join(format!("dp_ide_diff_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();

        let table_file = temp_dir.join("tables").join("items.sql");
        std::fs::write(&table_file, "CREATE TABLE items (id INTEGER PRIMARY KEY, title VARCHAR);").unwrap();

        let target_conn = Connection::open_in_memory().unwrap();

        // 1. Compute diff (should detect items table needs to be created)
        let diff_result = compute_ide_diff(&temp_dir, &target_conn).expect("compute_ide_diff failed");
        assert_eq!(diff_result.items.len(), 1);
        assert_eq!(diff_result.items[0].obj_name, "items");
        assert_eq!(diff_result.items[0].item_type, crate::diff::DiffItemType::CreateTable);

        // 2. Apply diff in target_conn
        apply_ide_diff(&temp_dir, &target_conn, &diff_result).expect("apply_ide_diff failed");

        // 3. Recompute diff (should be 0 changes now)
        let diff_after = compute_ide_diff(&temp_dir, &target_conn).expect("recompute failed");
        assert_eq!(diff_after.items.len(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_live_quack_compute_and_apply_ide_diff() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
            let temp_dir = std::env::temp_dir().join(format!("dp_quack_diff_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            let _ = std::fs::create_dir_all(temp_dir.join("tables"));

            // Ensure test table is dropped on remote
            let _ = conn.execute_batch("CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');");

            let table_file = temp_dir.join("tables").join("__duckpack_ide_quack_test_tbl.sql");
            std::fs::write(&table_file, "CREATE TABLE __duckpack_ide_quack_test_tbl (id INTEGER PRIMARY KEY, note VARCHAR);").unwrap();

            // 1. Compute diff against live Quack connection
            let diff_result = compute_ide_diff(&temp_dir, &conn).expect("compute_ide_diff failed on Quack");
            let pending_item = diff_result.items.iter().find(|i| i.obj_name == "__duckpack_ide_quack_test_tbl");
            assert!(pending_item.is_some(), "Expected pending create table for __duckpack_ide_quack_test_tbl");

            // 2. Filter diff result to only apply our test table
            let mut test_diff = diff_result.clone();
            for item in &mut test_diff.items {
                if item.obj_name != "__duckpack_ide_quack_test_tbl" {
                    item.selected = false;
                }
            }

            // 3. Apply diff via apply_ide_diff - this tests catalog resolution, transactions, and Quack DDL execution
            let apply_res = apply_ide_diff(&temp_dir, &conn, &test_diff);
            assert!(apply_res.is_ok(), "apply_ide_diff on Quack failed: {:?}", apply_res.err());

            // 4. Verify table exists on remote Quack database
            let count: i64 = conn.query_row(
                "SELECT * FROM target_db.query('SELECT count(*) FROM __duckpack_ide_quack_test_tbl')",
                [],
                |r| r.get(0),
            ).expect("Failed to query created test table on remote Quack");
            assert_eq!(count, 0);

            // Cleanup remote table
            let _ = conn.execute_batch("CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');");
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_diff_space_toggle_and_lock_recomputation() {
        let mut diff = crate::diff::DiffResult {
            items: vec![
                crate::diff::DiffItem {
                    item_type: crate::diff::DiffItemType::CreateTable,
                    obj_name: "customers".to_string(),
                    sql: "CREATE TABLE customers (id INT PRIMARY KEY);".to_string(),
                    old_def: None,
                    new_def: None,
                    selected: true,
                    locked: false,
                    ignored: false,
                },
                crate::diff::DiffItem {
                    item_type: crate::diff::DiffItemType::CreateView,
                    obj_name: "v_customers".to_string(),
                    sql: "CREATE VIEW v_customers AS SELECT * FROM customers;".to_string(),
                    old_def: None,
                    new_def: None,
                    selected: true,
                    locked: false,
                    ignored: false,
                },
            ],
        };

        // Initially both are selected and unlocked
        assert!(diff.items[0].selected);
        assert!(!diff.items[0].locked);
        assert!(diff.items[1].selected);
        assert!(!diff.items[1].locked);

        // Toggle customers off with Space
        diff.items[0].selected = false;
        crate::tui::recompute_locks(&mut diff.items);

        // customers is unselected
        assert!(!diff.items[0].selected);
        assert!(!diff.items[0].locked);

        // Dependent view v_customers is automatically locked and deselected
        assert!(!diff.items[1].selected);
        assert!(diff.items[1].locked);

        // Re-selecting customers unlocks the view
        diff.items[0].selected = true;
        crate::tui::recompute_locks(&mut diff.items);
        assert!(diff.items[0].selected);
        assert!(!diff.items[1].locked);
    }

    #[test]
    fn test_nested_schemas_in_ide_explorer() {
        let temp_dir = std::env::temp_dir().join(format!("dp_ide_nested_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        conn.execute("CREATE SCHEMA target_db.region;", []).unwrap();
        conn.execute("CREATE SCHEMA target_db.region.sales;", []).unwrap();
        conn.execute("CREATE TABLE target_db.region.products (id INT PRIMARY KEY);", []).unwrap();
        conn.execute("CREATE TABLE target_db.region.sales.orders (order_id INT PRIMARY KEY, qty INT);", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("db:target_db".to_string());
        expanded_groups.insert("target_db:Tables".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

        // Verify products node display
        let prod_node = nodes.iter().find(|n| n.name == "products").expect("products node");
        assert!(prod_node.display.contains("region.products"));

        // Verify orders node display with nested schema
        let orders_node = nodes.iter().find(|n| n.name == "orders").expect("orders node");
        assert!(orders_node.display.contains("region.sales.orders"));

        match &orders_node.node_type {
            NodeType::Table { database, schema, .. } => {
                assert_eq!(database.as_deref(), Some("target_db"));
                assert_eq!(schema.as_deref(), Some("region.sales"));
            }
            _ => panic!("Expected NodeType::Table"),
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_attach_modal_state_providers() {
        let mut modal = AttachModalState::new();
        assert_eq!(modal.provider(), AttachProvider::DuckDB);
        assert!(!modal.is_open);

        modal.reset();
        assert!(modal.is_open);
        assert_eq!(modal.active_field, 1);

        modal.set_provider(1);
        assert_eq!(modal.provider(), AttachProvider::Quack);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "9494");
        assert_eq!(modal.alias, "quack_remote");

        modal.set_provider(2);
        assert_eq!(modal.provider(), AttachProvider::MotherDuck);

        modal.set_provider(3);
        assert_eq!(modal.provider(), AttachProvider::SQLite);

        modal.set_provider(4);
        assert_eq!(modal.provider(), AttachProvider::PostgreSQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "5432");
        assert_eq!(modal.username, "postgres");

        modal.set_provider(5);
        assert_eq!(modal.provider(), AttachProvider::MySQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "3306");
        assert_eq!(modal.username, "root");

        modal.set_provider(6);
        assert_eq!(modal.provider(), AttachProvider::MSSQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "1433");
        assert_eq!(modal.username, "sa");

        // Test MSSQL Connection String building
        modal.database = "mydb".to_string();
        modal.alias = "mssql_db".to_string();
        modal.password = "secretpass".to_string();
        let (conn_str, alias) = modal.build_connection_target().unwrap();
        assert_eq!(alias, "mssql_db");
        assert!(conn_str.contains("Server=localhost,1433"));
        assert!(conn_str.contains("Database=mydb"));
        assert!(conn_str.contains("User Id=sa"));
        assert!(conn_str.contains("Password=secretpass"));
        assert!(conn_str.contains("TrustServerCertificate=yes"));

        modal.disable_ssl = true;
        let (conn_str_no_ssl, _) = modal.build_connection_target().unwrap();
        assert!(conn_str_no_ssl.contains("Encrypt=no"));

        // Test Postgres URI building
        modal.reset();
        modal.set_provider(4);
        modal.database = "analytics".to_string();
        modal.alias = "analytics".to_string();
        modal.password = "pgpass".to_string();
        modal.disable_ssl = true;
        let (pg_uri, pg_alias) = modal.build_connection_target().unwrap();
        assert_eq!(pg_alias, "analytics");
        assert_eq!(pg_uri, "postgres://postgres:pgpass@localhost:5432/analytics?sslmode=disable");

        // Test MySQL URI building
        modal.reset();
        modal.set_provider(5);
        modal.database = "store".to_string();
        modal.alias = "store".to_string();
        modal.password = "mysqlpass".to_string();
        modal.disable_ssl = true;
        let (mysql_uri, mysql_alias) = modal.build_connection_target().unwrap();
        assert_eq!(mysql_alias, "store");
        assert_eq!(mysql_uri, "mysql://root:mysqlpass@localhost:3306/store?ssl-mode=DISABLED");

        // Test Quack Target building
        modal.reset();
        modal.set_provider(1);
        let (quack_target, quack_alias) = modal.build_connection_target().unwrap();
        assert_eq!(quack_alias, "quack_remote");
        assert_eq!(quack_target, "quack:localhost:9494");

        // Test Typing and Backspace with auto-sync alias
        modal.reset();
        modal.set_provider(6);
        modal.active_field = 3; // Database Name field
        modal.handle_char('p');
        modal.handle_char('r');
        modal.handle_char('o');
        modal.handle_char('d');
        assert_eq!(modal.database, "prod");
        assert_eq!(modal.alias, "prod"); // Alias auto-populated
        modal.handle_backspace();
        assert_eq!(modal.database, "pro");
        assert_eq!(modal.alias, "pro");

        modal.active_field = 1; // Host field
        modal.host.clear();
        modal.handle_char('1');
        modal.handle_char('0');
        modal.handle_char('.');
        modal.handle_char('0');
        assert_eq!(modal.host, "10.0");
        modal.handle_backspace();
        assert_eq!(modal.host, "10.");

        // Test Navigation
        modal.active_field = 1;
        modal.nav_right();
        assert_eq!(modal.active_field, 2); // To Port
        modal.nav_left();
        assert_eq!(modal.active_field, 1); // Back to Host
        modal.nav_down();
        assert_eq!(modal.active_field, 3); // Down to Database Name

        modal.reset();
        assert!(!modal.disable_ssl);
        assert!(!modal.insecure);
        assert!(!modal.read_only);
    }

    #[test]
    fn test_secret_modal_state_and_providers() {
        let mut modal = SecretModalState::new();
        assert_eq!(modal.provider(), CloudProvider::S3);
        assert_eq!(modal.region, "us-east-1");
        assert!(modal.use_ssl);
        assert!(modal.save_to_env);

        modal.set_provider(1); // Cloudflare R2
        assert_eq!(modal.provider(), CloudProvider::CloudflareR2);
        assert_eq!(modal.region, "auto");
        assert!(modal.endpoint.contains("r2.cloudflarestorage.com"));

        modal.set_provider(2); // MinIO
        assert_eq!(modal.provider(), CloudProvider::MinIO);
        assert_eq!(modal.endpoint, "localhost:9000");
        assert!(!modal.use_ssl);

        modal.reset();
        assert_eq!(modal.provider(), CloudProvider::S3);
        assert!(modal.is_open);
    }

    #[test]
    fn test_explorer_nodes_secrets_and_cloud_buckets() {
        let conn = Connection::open_in_memory().unwrap();
        let meta = ExplorerMetadata {
            databases: vec![],
            active_database: "memory".to_string(),
            tables: vec![],
            table_counts: HashMap::new(),
            views: vec![],
            temp_tables: HashSet::new(),
            temp_views: HashSet::new(),
            macros: vec![],
            triggers: vec![],
            queries: vec![],
            dictionary: vec![],
            secrets: vec![("my_s3_secret".to_string(), "s3".to_string())],
            cloud_buckets: vec!["s3://analytics-lake".to_string()],
        };

        let mut expanded_groups = HashSet::new();
        let expanded_tables = HashSet::new();

        // Not expanded
        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let sec_group = nodes.iter().find(|n| n.name == "Secrets & Cloud Buckets").expect("Secrets & Cloud Buckets group");
        assert!(sec_group.display.contains("Secrets & Cloud (2)"));
        assert!(!nodes.iter().any(|n| n.name == "my_s3_secret"));

        // Expanded
        expanded_groups.insert("Secrets & Cloud Buckets".to_string());
        let nodes_expanded = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_secret"));
        assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_bucket"));
        assert!(nodes_expanded.iter().any(|n| n.name == "my_s3_secret"));
        assert!(nodes_expanded.iter().any(|n| n.name == "bucket:s3://analytics-lake"));
    }

    #[test]
    fn test_populate_secret_modal_for_edit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TEMPORARY TABLE duckdb_secrets (name VARCHAR, secret_string VARCHAR);", []).unwrap();
        conn.execute("INSERT INTO duckdb_secrets VALUES ('test_edit', 'name=test_edit;endpoint=localhost:9000;key_id=my_key;use_ssl=false');", []).unwrap();

        let temp_dir = std::env::temp_dir().join(format!("dp_ide_edit_sec_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let _ = std::fs::write(temp_dir.join(".env"), "AWS_SECRET_ACCESS_KEY=my_secret_val\nDUCKPACK_S3_BUCKET=s3://test-bucket\n");

        let mut modal = SecretModalState::new();
        populate_secret_modal_for_edit(&conn, &temp_dir, "test_edit", &mut modal);

        assert!(modal.is_open);
        assert_eq!(modal.name, "test_edit");
        assert_eq!(modal.key_id, "my_key");
        assert_eq!(modal.secret_key, "my_secret_val");
        assert_eq!(modal.endpoint, "localhost:9000");
        assert_eq!(modal.bucket, "s3://test-bucket");
        assert_eq!(modal.provider(), CloudProvider::MinIO);
        assert!(!modal.use_ssl);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_database_picker_modal_state() {
        let mut modal = DatabasePickerModalState::new();
        assert!(!modal.is_open);
        assert_eq!(modal.selected_index, 0);

        let dbs = vec![
            crate::catalog::DatabaseInfo {
                name: "memory".to_string(),
                db_type: "duckdb".to_string(),
                path: None,
                is_active: true,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
            crate::catalog::DatabaseInfo {
                name: "analytics_db".to_string(),
                db_type: "postgres".to_string(),
                path: Some("postgres://...".to_string()),
                is_active: false,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
            crate::catalog::DatabaseInfo {
                name: "mssql_db".to_string(),
                db_type: "mssql".to_string(),
                path: None,
                is_active: false,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
        ];

        modal.open_for_current("analytics_db", &dbs);
        assert!(modal.is_open);
        assert_eq!(modal.selected_index, 1);

        modal.next(dbs.len());
        assert_eq!(modal.selected_index, 2);

        // Next at end should clamp to last element
        modal.next(dbs.len());
        assert_eq!(modal.selected_index, 2);

        modal.prev();
        assert_eq!(modal.selected_index, 1);

        modal.prev();
        assert_eq!(modal.selected_index, 0);

        // Prev at beginning should clamp to 0
        modal.prev();
        assert_eq!(modal.selected_index, 0);

        // Opening for unknown db falls back to index 0
        modal.open_for_current("unknown_catalog", &dbs);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_editor_tab_target_database_and_db_icon() {
        let tab = EditorTab::new("mssql_db".to_string());
        assert_eq!(tab.target_database, "mssql_db");
        assert!(!tab.is_dirty);
        assert!(tab.active_file.is_none());

        assert_eq!(db_icon("sqlite"), "🗃️");
        assert_eq!(db_icon("postgres"), "🔌");
        assert_eq!(db_icon("postgresql"), "🔌");
        assert_eq!(db_icon("mysql"), "🔌");
        assert_eq!(db_icon("mssql"), "🔌");
        assert_eq!(db_icon("duckdb"), "📁");
        assert_eq!(db_icon("unknown"), "📁");
    }

    #[test]
    fn test_exit_confirmation_modal_state() {
        let mut modal = ExitConfirmationModalState::new();
        assert!(!modal.is_open);
        assert!(!modal.confirm_exit);

        modal.open();
        assert!(modal.is_open);
        assert!(!modal.confirm_exit); // Safe default: Cancel

        modal.toggle();
        assert!(modal.confirm_exit); // Toggled to Exit

        modal.toggle();
        assert!(!modal.confirm_exit); // Toggled back to Cancel

        modal.close();
        assert!(!modal.is_open);
        assert!(!modal.confirm_exit);
    }
}

