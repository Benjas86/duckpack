use duckdb::Connection;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::catalog::{format_row_count, load_table_columns_for, ExplorerMetadata};

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
    Sequence,
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

/// Formats the database provider icon based on database type.
pub(crate) fn db_icon(db_type: &str) -> &'static str {
    match db_type.to_lowercase().as_str() {
        "sqlite" => "🗃️",
        "postgres" | "postgresql" | "mysql" | "mssql" => "🔌",
        _ => "📁",
    }
}

/// Formats the display line for a column in the database explorer tree.
pub(crate) fn format_column_display(
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

/// Returns the semantic highlight `Style` for a DuckDB column data type in the Explorer tree.
pub(crate) fn column_data_type_style(data_type: &str) -> Style {
    let upper = data_type.trim().to_uppercase();
    let base = upper
        .split(|c: char| c == '(' || c == '[' || c.is_whitespace())
        .next()
        .unwrap_or(&upper);

    if upper.ends_with("[]")
        || matches!(
            base,
            "LIST" | "ARRAY" | "STRUCT" | "MAP" | "UNION" | "JSON" | "VARIANT" | "GEOMETRY" | "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" | "BIT" | "BITSTRING"
        )
    {
        Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD)
    } else if matches!(
        base,
        "TINYINT"
            | "INT1"
            | "SMALLINT"
            | "INT2"
            | "SHORT"
            | "INTEGER"
            | "INT"
            | "INT4"
            | "SIGNED"
            | "BIGINT"
            | "INT8"
            | "LONG"
            | "HUGEINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UINT"
            | "UBIGINT"
            | "UHUGEINT"
            | "FLOAT"
            | "FLOAT4"
            | "REAL"
            | "DOUBLE"
            | "FLOAT8"
            | "DECIMAL"
            | "NUMERIC"
    ) {
        Style::default().fg(Color::LightGreen)
    } else if matches!(
        base,
        "VARCHAR" | "CHAR" | "BPCHAR" | "TEXT" | "STRING" | "UUID" | "ENUM"
    ) {
        Style::default().fg(Color::Yellow)
    } else if matches!(
        base,
        "DATE"
            | "TIME"
            | "TIMETZ"
            | "TIMESTAMP"
            | "TIMESTAMPTZ"
            | "TIMESTAMP_S"
            | "TIMESTAMP_MS"
            | "TIMESTAMP_NS"
            | "INTERVAL"
    ) {
        Style::default().fg(Color::LightMagenta)
    } else if matches!(base, "BOOLEAN" | "BOOL") {
        Style::default().fg(Color::Magenta)
    } else {
        Style::default().fg(Color::Cyan)
    }
}

///Slices a sequence of styled `Span`s horizontally by skipping `skip_chars` characters from the left.
pub(crate) fn slice_spans_horizontally(
    spans: Vec<Span<'static>>,
    skip_chars: usize,
) -> Line<'static> {
    if skip_chars == 0 {
        return Line::from(spans);
    }
    let mut remaining_skip = skip_chars;
    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let content_chars: Vec<char> = span.content.chars().collect();
        if remaining_skip >= content_chars.len() {
            remaining_skip -= content_chars.len();
        } else if remaining_skip > 0 {
            let sliced: String = content_chars[remaining_skip..].iter().collect();
            remaining_skip = 0;
            out.push(Span::styled(sliced, span.style));
        } else {
            out.push(span);
        }
    }
    Line::from(out)
}

/// Renders an `ExplorerNode` into a styled Ratatui `Line`, applying semantic colour coding to column types and PK/FK badges.
pub(crate) fn render_explorer_node_line(
    item: &ExplorerNode,
    horizontal_scroll: usize,
) -> Line<'static> {
    let indent = "  ".repeat(item.level);
    let expand_indicator = if item.is_expandable {
        if item.is_expanded {
            "▼ "
        } else {
            "▶ "
        }
    } else {
        ""
    };

    if let NodeType::Column {
        ref data_type,
        is_pk,
        is_fk,
        ref fk_target,
    } = item.node_type
    {
        // Determine inner tree indent from `item.display` up to `└─ `
        let tree_prefix = item
            .display
            .find("└─ ")
            .map(|idx| &item.display[..idx])
            .unwrap_or("");
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(
            format!("{}{}{}└─ ", indent, expand_indicator, tree_prefix),
            Style::default().fg(Color::DarkGray),
        ));
        if is_pk && is_fk {
            spans.push(Span::raw("🔑🔗 "));
        } else if is_pk {
            spans.push(Span::raw("🔑 "));
        } else if is_fk {
            spans.push(Span::raw("🔗 "));
        }
        spans.push(Span::styled(
            format!("{} ", item.name),
            if is_pk {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            },
        ));
        spans.push(Span::styled("(", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            data_type.clone(),
            column_data_type_style(data_type),
        ));
        spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));

        if is_pk && is_fk {
            let target = fk_target.as_deref().unwrap_or("unknown");
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                "[PK, ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!("FK -> {}]", target),
                Style::default().fg(Color::Cyan),
            ));
        } else if is_pk {
            spans.push(Span::styled(
                " [PK]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if is_fk {
            let target = fk_target.as_deref().unwrap_or("unknown");
            spans.push(Span::styled(
                format!(" [FK -> {}]", target),
                Style::default().fg(Color::Cyan),
            ));
        }

        return slice_spans_horizontally(spans, horizontal_scroll);
    }

    let display_text = format!("{}{}{}", indent, expand_indicator, item.display)
        .chars()
        .skip(horizontal_scroll)
        .collect::<String>();
    Line::from(display_text)
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
        ProjectCategory::Sequence => format!(
            "-- Sequence: {}\nCREATE SEQUENCE IF NOT EXISTS {} START 1;\n",
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
        ProjectCategory::Seed => "id,name,value\n1,sample,100\n".to_string(),
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
        ProjectCategory::Sequence => "sequences",
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

    let categories: &[(&str, &str, &str, Option<&str>, ProjectCategory)] = &[
        ("schemas", "proj:schemas", "📁 Schemas", Some("sql"), ProjectCategory::Schema),
        ("sequences", "proj:sequences", "🔢 Sequences", Some("sql"), ProjectCategory::Sequence),
        ("tables", "proj:tables", "📦 Tables", Some("sql"), ProjectCategory::Table),
        ("views", "proj:views", "👁️ Views", Some("sql"), ProjectCategory::View),
        ("macros", "proj:macros", "🔧 Macros", Some("sql"), ProjectCategory::Macro),
        ("triggers", "proj:triggers", "⚡ Triggers", Some("sql"), ProjectCategory::Trigger),
        ("queries", "proj:queries", "📄 Queries", Some("sql"), ProjectCategory::Query),
        ("seeds", "proj:seeds", "🌱 Seeds", None, ProjectCategory::Seed),
    ];

    for &(subdir, group_key, title_prefix, ext_filter, category) in categories {
        let files = scan_dir(subdir, ext_filter);
        let is_expanded = expanded_groups.contains(group_key);
        items.push(ExplorerNode {
            name: group_key.to_string(),
            display: format!("{} ({})", title_prefix, files.len()),
            level: 0,
            is_expandable: true,
            is_expanded,
            node_type: NodeType::Group,
        });
        if is_expanded {
            if files.is_empty() {
                items.push(ExplorerNode {
                    name: format!("{}:empty", group_key),
                    display: "  └─ (empty - press 'a' to add)".to_string(),
                    level: 1,
                    is_expandable: false,
                    is_expanded: false,
                    node_type: NodeType::Group,
                });
            } else {
                for p in &files {
                    let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let badge = git_status.get(p).map(|b| format!(" {}", b)).unwrap_or_default();
                    items.push(ExplorerNode {
                        name: fname.clone(),
                        display: format!("  ├─ {}{}", fname, badge),
                        level: 1,
                        is_expandable: false,
                        is_expanded: false,
                        node_type: NodeType::ProjectFile {
                            path: p.clone(),
                            category,
                        },
                    });
                }
            }
        }
    }

    // Config files
    let config_candidates = [
        "duckpack.toml",
        ".env",
        ".duckpackignore",
        ".duckdbignore",
        ".duckignore",
    ];
    for c in &config_candidates {
        let p = project_dir.join(c);
        if p.exists() {
            let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let badge = git_status.get(&p).map(|b| format!(" {}", b)).unwrap_or_default();
            items.push(ExplorerNode {
                name: fname.clone(),
                display: format!("⚙️ {}{}", fname, badge),
                level: 0,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::ProjectFile {
                    path: p,
                    category: ProjectCategory::Config,
                },
            });
        }
    }

    items
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
                            let is_tbl_expanded = expanded_tables.contains(&table_key)
                                || expanded_tables.contains(&t.display_name())
                                || expanded_tables.contains(&t.name);
                            let count_str = t
                                .row_count
                                .map(|c| format!(" ({})", format_row_count(c)))
                                .unwrap_or_default();
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
                                    let display = format_column_display(
                                        "      ",
                                        &col.name,
                                        &col.data_type,
                                        col.is_pk,
                                        col.is_fk,
                                        col.fk_target.as_deref(),
                                    );
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
                            let is_view_expanded = expanded_tables.contains(&view_key)
                                || expanded_tables.contains(&v.display_name())
                                || expanded_tables.contains(&v.name);
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
                    let count_str = meta
                        .table_counts
                        .get(t)
                        .map(|&c| format!(" ({})", format_row_count(c)))
                        .unwrap_or_default();
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
                            let display = format_column_display(
                                "    ",
                                &col.name,
                                &col.data_type,
                                col.is_pk,
                                col.is_fk,
                                col.fk_target.as_deref(),
                            );
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
                node_type: NodeType::CloudBucket {
                    bucket: bucket.clone(),
                },
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
                        let short_err = if err_msg.contains("403")
                            || err_msg.contains("SignatureDoesNotMatch")
                            || err_msg.contains("InvalidAccessKeyId")
                        {
                            "⚠️ Auth failed: Check Key/Secret (press 'e' to edit)"
                        } else if err_msg.contains("404") || err_msg.contains("NoSuchBucket") {
                            "⚠️ Bucket not found (press 'e' to edit URI)"
                        } else if err_msg.contains("Malformed S3 list") {
                            "⚠️ Malformed S3 response (URL_STYLE path required)"
                        } else if err_msg.contains("Connection refused")
                            || err_msg.contains("Could not resolve host")
                        {
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
        let icon = if *is_dir {
            "📁"
        } else if filename.ends_with(".parquet") {
            "📊"
        } else if filename.ends_with(".csv") {
            "📝"
        } else {
            "📄"
        };
        let dir_key = format!("bucket_dir:{}", obj_path);
        let is_expanded = *is_dir && expanded_groups.contains(&dir_key);
        items.push(ExplorerNode {
            name: obj_path.clone(),
            display: format!("{}{} {}", indent, icon, filename),
            level,
            is_expandable: *is_dir,
            is_expanded,
            node_type: NodeType::CloudFile {
                path: obj_path.clone(),
                is_dir: *is_dir,
            },
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

