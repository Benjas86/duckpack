use crate::diff::{self, CatalogSnapshot, DiffItem, DiffItemType, DiffResult};
use crate::engine::{self, DbExecutor};
use crate::syntax::highlight_sql;
use anyhow::Result;
use duckdb::Connection;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::event::{self, Event, KeyCode},
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

/// Defines the various actions that can be triggered by the user within the TUI.
pub enum TuiAction {
    Apply,
    Refresh,
    Quit,
    ToggleDestructiveMode,
    Inspect { obj_name: String },
    CloseInspect,
    Pull { obj_name: String, item_type: DiffItemType },
    Explore,
    CopyStatus,
    SwitchCatalog,
}

/// Summary returned when an interactive `DiffReviewSession` completes.
#[derive(Debug, Clone, Default)]
pub struct DiffReviewSummary {
    pub total_applied_count: usize,
    pub applied_items: Vec<String>,
    pub applied_catalogs: Vec<String>,
}

/// Returns the compact badge and style for a `DiffItemType` (shared between TUI and IDE modal).
pub fn diff_item_compact_badge(item_type: &DiffItemType) -> (&'static str, Style) {
    match item_type {
        DiffItemType::CreateTable
        | DiffItemType::CreateSchema
        | DiffItemType::CreateSequence
        | DiffItemType::CreateView
        | DiffItemType::CreateMacro
        | DiffItemType::CreateTrigger => ("[+]", Style::default().fg(Color::Green)),
        DiffItemType::AlterTable
        | DiffItemType::AddConstraint
        | DiffItemType::RenameTable
        | DiffItemType::RenameColumn => ("[~]", Style::default().fg(Color::Yellow)),
        DiffItemType::RecreateTable => (
            "[⚡]",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ),
        DiffItemType::DropTable
        | DiffItemType::DropSchema
        | DiffItemType::DropSequence
        | DiffItemType::DropView
        | DiffItemType::DropMacro
        | DiffItemType::DropColumn
        | DiffItemType::DropTrigger => ("[-]", Style::default().fg(Color::Red)),
    }
}

/// Recalculates the locked state of all diff items based on their dependencies.
/// If a parent item (like a CREATE TABLE) is deselected, dependent items (like ALTER TABLE or INSERT)
/// are automatically locked and deselected to prevent invalid SQL execution.
pub(crate) fn recompute_locks(items: &mut Vec<DiffItem>) {
    for item in items.iter_mut() {
        item.locked = false;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..items.len() {
            if !items[i].selected {
                let obj_name = items[i].obj_name.clone();
                for j in 0..items.len() {
                    if i != j
                        && (!items[j].locked || items[j].selected)
                        && items[j].sql.contains(&obj_name)
                    {
                        items[j].selected = false;
                        items[j].locked = true;
                        changed = true;
                    }
                }
            }
        }
    }
}

/// Wraps the `sqlformat` crate to standardize formatting for SQL diffing.
/// We use this to ensure that whitespace or casing differences in `.sql` files
/// don't trigger false positive diffs.
pub(crate) fn format_sql_string(sql: &str) -> String {
    let mut options = sqlformat::FormatOptions::default();
    options.indent = sqlformat::Indent::Spaces(4);
    options.uppercase = Some(true);
    options.lines_between_queries = 1;

    sqlformat::format(sql, &sqlformat::QueryParams::None, &options)
}

/// Generates a side-by-side terminal diff view between two SQL definitions.
/// It highlights added lines in Green and deleted lines in Red,
/// while leaving unchanged lines with standard syntax highlighting.
pub(crate) fn get_side_by_side_diff(
    old_def: &str,
    new_def: &str,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let old_formatted = format_sql_string(old_def);
    let new_formatted = format_sql_string(new_def);

    let diff = TextDiff::from_lines(&old_formatted, &new_formatted);
    let mut left_lines = Vec::new();
    let mut right_lines = Vec::new();

    for change in diff.iter_all_changes() {
        let val = change.value();
        let val_trim = if val.ends_with('\n') {
            &val[..val.len() - 1]
        } else {
            val
        };

        match change.tag() {
            ChangeTag::Equal => {
                let mut spans = vec![Span::styled("  ", Style::default().fg(Color::DarkGray))];
                spans.extend(highlight_sql(val_trim));
                left_lines.push(Line::from(spans.clone()));
                right_lines.push(Line::from(spans));
            }
            ChangeTag::Delete => {
                let mut spans = vec![Span::styled(
                    "- ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )];
                spans.push(Span::styled(
                    val_trim.to_string(),
                    Style::default().fg(Color::Red),
                ));
                left_lines.push(Line::from(spans));
                right_lines.push(Line::from(vec![Span::raw("")]));
            }
            ChangeTag::Insert => {
                left_lines.push(Line::from(vec![Span::raw("")]));
                let mut spans = vec![Span::styled(
                    "+ ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )];
                spans.push(Span::styled(
                    val_trim.to_string(),
                    Style::default().fg(Color::Green),
                ));
                right_lines.push(Line::from(spans));
            }
        }
    }

    (left_lines, right_lines)
}

/// Sorts the execution order of `DiffItem`s to satisfy SQL dependencies.
/// `CREATE` statements execute first, followed by `ALTER`, and finally `DROP` statements.
pub(crate) fn sort_diff_items(items: &mut Vec<DiffItem>) {
    items.sort_by_key(|item| match item.item_type {
        DiffItemType::CreateSchema => 0,
        DiffItemType::CreateSequence => 1,
        DiffItemType::CreateTable
        | DiffItemType::CreateView
        | DiffItemType::CreateMacro
        | DiffItemType::CreateTrigger => 2,
        DiffItemType::AlterTable
        | DiffItemType::RenameTable
        | DiffItemType::RenameColumn
        | DiffItemType::AddConstraint => 3,
        DiffItemType::DropTrigger
        | DiffItemType::DropView
        | DiffItemType::DropTable
        | DiffItemType::DropMacro
        | DiffItemType::DropColumn
        | DiffItemType::DropSequence
        | DiffItemType::DropSchema
        | DiffItemType::RecreateTable => 4,
    });
}

/// Deep controller owning the state and actions of an interactive declarative diff review session.
///
/// Works identically across CLI (`duckpack apply` / `duckpack diff -i`), Python (`duckpack.apply(interactive=True)`),
/// and headless unit tests via `&mut dyn DbExecutor`.
pub struct DiffReviewSession {
    pub project_dir: PathBuf,
    pub db_target: String,
    pub active_catalog: String,
    pub force_drop: bool,
    pub status_msg: String,
    pub inspect_data: Option<(String, Vec<String>, Vec<Vec<String>>, i64)>,
    pub target_snapshot: Option<CatalogSnapshot>,
    pub diff_result: DiffResult,
    pub summary: DiffReviewSummary,
}

impl DiffReviewSession {
    /// Initializes a `DiffReviewSession` by validating the target catalog and computing the initial diff.
    pub fn new(
        exec: &mut dyn DbExecutor,
        project_dir: &Path,
        db_target: &str,
        initial_catalog: Option<&str>,
        force_drop: bool,
    ) -> Result<Self> {
        let active_catalog = diff::validate_target_catalog_exec(exec, initial_catalog, true)?;
        let mut session = Self {
            project_dir: project_dir.to_path_buf(),
            db_target: db_target.to_string(),
            active_catalog,
            force_drop,
            status_msg: String::from(
                "Ready. Press 'Enter' to Apply, 'r' to Refresh, 'b' to switch catalog, 'q' to Quit.",
            ),
            inspect_data: None,
            target_snapshot: None,
            diff_result: DiffResult { items: Vec::new() },
            summary: DiffReviewSummary::default(),
        };
        session.refresh(exec)?;
        Ok(session)
    }

    /// Recomputes `target_snapshot` and `diff_result` from disk and the target catalog.
    pub fn refresh(&mut self, exec: &mut dyn DbExecutor) -> Result<()> {
        let (_shadow_conn, shadow_snapshot) = CatalogSnapshot::from_project_dir(&self.project_dir)?;
        let target_snapshot = CatalogSnapshot::introspect(exec, Some(&self.active_catalog))?;
        let ignore_list = engine::load_ignore_rules(&self.project_dir);
        self.diff_result = diff::compute_diff_snapshots(
            &shadow_snapshot,
            &target_snapshot,
            &self.project_dir,
            &ignore_list,
        );
        sort_diff_items(&mut self.diff_result.items);
        self.target_snapshot = Some(target_snapshot);
        Ok(())
    }

    /// Formats the title displayed in the top-right header of the diff review TUI.
    pub fn display_title(&self) -> String {
        let display_db = if self.db_target.starts_with("quack:localhost:")
            || self.db_target.starts_with("quack:127.0.0.1:")
        {
            format!("{} (Python In-Memory)", self.db_target)
        } else {
            self.db_target.clone()
        };
        format!("{} [{}]", display_db, self.active_catalog)
    }

    /// Generates a dry-run SQL migration script from the current `diff_result`.
    pub fn render_dry_run_script(&self) -> String {
        let mut script = String::new();
        for item in &self.diff_result.items {
            if item.is_destructive() && !self.force_drop {
                continue;
            }
            if !item.sql.trim().is_empty() {
                script.push_str(&item.sql);
                if !item.sql.trim().ends_with(';') {
                    script.push(';');
                }
                script.push_str("\n\n");
            }
        }
        script
    }

    /// Toggles destructive mode (`force_drop`).
    pub fn toggle_destructive_mode(&mut self) {
        self.force_drop = !self.force_drop;
        self.status_msg = if self.force_drop {
            "Destructive Mode ENABLED. Drops will be executed.".to_string()
        } else {
            "Destructive Mode DISABLED. Drops will be ignored.".to_string()
        };
    }

    /// Writes the current `status_msg` to `duckpack.log`.
    pub fn copy_status_to_log(&mut self) {
        let _ = std::fs::write("duckpack.log", &self.status_msg);
        self.status_msg = "Status text copied to duckpack.log!".to_string();
    }

    /// Cycles `active_catalog` across attached writable DuckDB/Quack catalogs and refreshes the diff.
    pub fn switch_catalog(&mut self, exec: &mut dyn DbExecutor) -> Result<()> {
        let attached = diff::list_attached_databases_exec(exec).unwrap_or_default();
        let writable_dbs: Vec<_> = attached
            .into_iter()
            .filter(|d| {
                !d.readonly
                    && (d.db_type.eq_ignore_ascii_case("duckdb")
                        || d.db_type.eq_ignore_ascii_case("quack"))
            })
            .collect();
        if writable_dbs.len() > 1 {
            let curr_idx = writable_dbs
                .iter()
                .position(|d| d.name.eq_ignore_ascii_case(&self.active_catalog))
                .unwrap_or(0);
            let next_idx = (curr_idx + 1) % writable_dbs.len();
            self.active_catalog = writable_dbs[next_idx].name.clone();
            self.refresh(exec)?;
            self.status_msg = format!("Switched target catalog to '{}'", self.active_catalog);
        } else {
            self.status_msg = format!(
                "Catalog '{}' is the only writable DuckDB database attached.",
                self.active_catalog
            );
        }
        Ok(())
    }

    /// Queries up to 5 sample rows and the total row count of `obj_name` for the TUI inspect popup.
    pub fn inspect_object(&mut self, exec: &mut dyn DbExecutor, obj_name: &str) {
        let safe_obj = obj_name.replace('\'', "''");
        let quoted_obj = diff::quote_qualified_identifier(obj_name);

        let mut cols = Vec::new();
        if let Ok((_, pragma_rows)) =
            exec.query_rows(&format!("PRAGMA table_info('{}')", safe_obj))
        {
            for row in pragma_rows {
                if let Some(col_name) = row.get(1) {
                    if !col_name.is_empty() && col_name != "NULL" {
                        cols.push(col_name.clone());
                    }
                }
            }
        }

        if cols.is_empty() {
            self.status_msg = format!(
                "Cannot inspect '{}': Does not exist in target database yet.",
                obj_name
            );
            return;
        }

        let mut row_count: i64 = 0;
        if let Ok((_, count_rows)) =
            exec.query_rows(&format!("SELECT count(*) FROM {}", quoted_obj))
        {
            if let Some(first_row) = count_rows.first() {
                if let Some(val) = first_row.first() {
                    row_count = val.parse::<i64>().unwrap_or(0);
                }
            }
        }

        let cast_cols = cols
            .iter()
            .map(|c| format!("CAST(\"{}\" AS VARCHAR)", c.replace('\"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", ");
        if let Ok((_, rows)) = exec.query_rows(&format!(
            "SELECT {} FROM {} LIMIT 5",
            cast_cols, quoted_obj
        )) {
            self.inspect_data = Some((obj_name.to_string(), cols, rows, row_count));
        }
    }

    /// Reverse-engineers `obj_name` from `target_snapshot` into the local project directory and refreshes the diff.
    pub fn pull_object(
        &mut self,
        exec: &mut dyn DbExecutor,
        obj_name: &str,
        item_type: &DiffItemType,
    ) -> Result<()> {
        if self.target_snapshot.is_none() {
            self.refresh(exec)?;
        }
        let pull_res = self
            .target_snapshot
            .as_ref()
            .unwrap()
            .pull_object_to_project(&self.project_dir, item_type, obj_name);
        match pull_res {
            Ok(_) => {
                self.refresh(exec)?;
                self.status_msg = format!(
                    "Successfully pulled '{}' back into local project!",
                    obj_name
                );
            }
            Err(e) => {
                self.status_msg = format!("Error pulling '{}': {}", obj_name, e);
            }
        }
        Ok(())
    }

    /// Applies selected diff items to `exec` (with automatic `.duckdb.bak` safety backup/restore for local files)
    /// and refreshes the session snapshot.
    pub fn apply_selected(&mut self, exec: &mut dyn DbExecutor) -> Result<usize> {
        let remote = engine::is_remote(&self.db_target);
        let is_quack = self.db_target.starts_with("quack:");
        let applied_items: Vec<String> = self
            .diff_result
            .items
            .iter()
            .filter(|i| i.selected && !i.ignored && (!i.is_destructive() || self.force_drop))
            .map(|i| i.obj_name.clone())
            .collect();
        let applied_count = applied_items.len();

        let backup_path = if !remote && !is_quack && self.db_target != ":memory:" {
            let db_path = PathBuf::from(&self.db_target);
            if db_path.exists() {
                let mut b_path = db_path.clone();
                b_path.set_extension("duckdb.bak");
                let _ = std::fs::copy(&db_path, &b_path);
                Some((db_path, b_path))
            } else {
                None
            }
        } else {
            None
        };

        let apply_res = engine::apply_diff_with_executor(
            exec,
            &self.diff_result,
            self.force_drop,
            remote,
            &self.project_dir,
            Some(&self.active_catalog),
        );

        match apply_res {
            Ok(_) => {
                if let Some((_, b_path)) = backup_path {
                    if b_path.exists() {
                        let _ = std::fs::remove_file(&b_path);
                    }
                }
                self.summary.total_applied_count += applied_count;
                self.summary.applied_items.extend(applied_items);
                if applied_count > 0 && !self.summary.applied_catalogs.contains(&self.active_catalog)
                {
                    self.summary
                        .applied_catalogs
                        .push(self.active_catalog.clone());
                }
                self.refresh(exec)?;
                self.status_msg = format!(
                    "✓ Successfully applied {} change(s) to '{}'! Press 'b' to switch catalog, or 'q' to finish.",
                    applied_count, self.active_catalog
                );
                Ok(applied_count)
            }
            Err(e) => {
                if let Some((db_path, b_path)) = backup_path {
                    if b_path.exists() {
                        let _ = std::fs::copy(&b_path, &db_path);
                        let _ = std::fs::remove_file(&b_path);
                    }
                }
                self.status_msg = format!("Error applying schema: {}", e);
                Err(e)
            }
        }
    }
}

/// Runs the full interactive diff review TUI session against `conn`, handling all `TuiAction` dispatches
/// (`SwitchCatalog`, `Inspect`, `Pull`, `ToggleDestructiveMode`, `Explore`, `Apply`, and `Quit`).
pub fn run_diff_review_session(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    conn: &Connection,
    project_dir: &Path,
    db_target: &str,
    initial_catalog: Option<&str>,
    force_drop: bool,
    quack_disable_ssl: bool,
) -> Result<DiffReviewSummary> {
    let mut exec = conn;
    let mut session =
        DiffReviewSession::new(&mut exec, project_dir, db_target, initial_catalog, force_drop)?;
    let project_dir_buf = project_dir.to_path_buf();

    loop {
        let title = session.display_title();
        let action = draw_and_handle_events(
            terminal,
            &mut session.diff_result,
            session.force_drop,
            &session.status_msg,
            false,
            &session.inspect_data,
            &title,
        )?;

        let mut exec = conn;
        match action {
            TuiAction::Quit => break,
            TuiAction::SwitchCatalog => {
                session.switch_catalog(&mut exec)?;
            }
            TuiAction::CopyStatus => {
                session.copy_status_to_log();
            }
            TuiAction::Refresh => {
                session.refresh(&mut exec)?;
                session.status_msg = "Refreshed schema from disk.".to_string();
            }
            TuiAction::ToggleDestructiveMode => {
                session.toggle_destructive_mode();
            }
            TuiAction::Inspect { obj_name } => {
                session.inspect_object(&mut exec, &obj_name);
            }
            TuiAction::CloseInspect => {
                session.inspect_data = None;
            }
            TuiAction::Explore => {
                crate::ide::run_ide_loop(
                    terminal,
                    conn,
                    &project_dir_buf,
                    db_target,
                    quack_disable_ssl,
                )?;
                terminal.clear()?;
                session.refresh(&mut exec)?;
            }
            TuiAction::Pull {
                obj_name,
                item_type,
            } => {
                session.pull_object(&mut exec, &obj_name, &item_type)?;
            }
            TuiAction::Apply => {
                session.status_msg = "Applying changes...".to_string();
                let _ = draw_and_handle_events(
                    terminal,
                    &mut session.diff_result,
                    session.force_drop,
                    &session.status_msg,
                    true,
                    &session.inspect_data,
                    &title,
                );
                let _ = session.apply_selected(&mut exec);
            }
        }
    }

    Ok(session.summary)
}

/// The main execution loop for the `apply` command's Terminal UI.
/// Handles rendering the list of changes, capturing keyboard events, and toggling selection states.
pub fn draw_and_handle_events(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    diff: &mut DiffResult,
    force_drop: bool,
    status_msg: &str,
    draw_only: bool,
    inspect_data: &Option<(String, Vec<String>, Vec<Vec<String>>, i64)>,
    db_path: &str,
) -> Result<TuiAction> {
    sort_diff_items(&mut diff.items);

    let mut list_state = ListState::default();
    let mut scroll_position: u16 = 0;
    let mut show_help = false;
    let mut show_ignored = false;
    let mut inspect_scroll: u16 = 0;
    let mut max_inspect_scroll: u16 = 0;
    let mut exit_confirm_modal = crate::ide::ExitConfirmationModalState::new();
    let mut current_exit_dialog_rect = ratatui::layout::Rect::default();
    if !diff.items.is_empty() {
        list_state.select(Some(0));
    }

    loop {
        let visible_items: Vec<(usize, &DiffItem)> = diff
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| show_ignored || !item.ignored)
            .collect();
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Length(3),      // Top Header
                        Constraint::Length(4),      // Status Message
                        Constraint::Percentage(50), // List
                        Constraint::Percentage(50), // Details
                        Constraint::Length(3),      // Footer
                    ]
                    .as_ref(),
                )
                .split(f.area());

            let top_header_text = vec![Span::styled(
                format!(
                    " DuckPack v{} - Diff Mode ",
                    crate::engine::duckpack_version()
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )];
            let top_header = Paragraph::new(Line::from(top_header_text)).block(
                Block::default().borders(Borders::ALL).title_top(
                    Line::from(format!(" Target DB: {} ", db_path))
                        .alignment(Alignment::Right)
                        .style(Style::default().fg(Color::Cyan)),
                ),
            );
            f.render_widget(top_header, chunks[0]);

            let mut status_text = vec![];
            if force_drop {
                status_text.push(Span::styled(
                    "[WARNING: DESTRUCTIVE MODE ACTIVE] ",
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::RAPID_BLINK),
                ));
            }
            if !status_msg.is_empty() {
                status_text.push(Span::styled(status_msg, Style::default().fg(Color::Green)));
            }

            let status_block = Paragraph::new(Line::from(status_text))
                .block(Block::default().borders(Borders::ALL).title("Status"))
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(status_block, chunks[1]);

            if diff.is_empty() {
                let p = Paragraph::new(Span::styled(
                    "No schema changes detected. Database is up to date.",
                    Style::default().fg(Color::Green),
                ))
                .block(Block::default().borders(Borders::ALL));
                f.render_widget(p, chunks[4]);
            } else {
                let items: Vec<ListItem> = visible_items
                    .iter()
                    .map(|(_, item)| {
                        let checkbox = if item.locked {
                            "[-] "
                        } else if item.selected {
                            "[x] "
                        } else {
                            "[ ] "
                        };

                        let (mut prefix, mut style) = match item.item_type {
                            DiffItemType::CreateSchema => {
                                ("[NEW SCHEMA] ", Style::default().fg(Color::Cyan))
                            }
                            DiffItemType::DropSchema => {
                                if force_drop {
                                    ("[DROP SCHEMA] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED SCHEMA] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::CreateTable => {
                                ("[NEW TABLE] ", Style::default().fg(Color::Green))
                            }
                            DiffItemType::AlterTable => {
                                ("[ALTER TABLE] ", Style::default().fg(Color::Yellow))
                            }
                            DiffItemType::RenameTable => {
                                ("[RENAME TABLE] ", Style::default().fg(Color::LightBlue))
                            }
                            DiffItemType::RenameColumn => {
                                ("[RENAME COL] ", Style::default().fg(Color::LightBlue))
                            }
                            DiffItemType::DropTable => {
                                if force_drop {
                                    ("[DROP TABLE] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::DropColumn => {
                                if force_drop {
                                    ("[DROP COL] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::CreateView => {
                                ("[VIEW] ", Style::default().fg(Color::Blue))
                            }
                            DiffItemType::DropView => {
                                if force_drop {
                                    ("[DROP VIEW] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::CreateMacro => {
                                ("[MACRO] ", Style::default().fg(Color::Magenta))
                            }
                            DiffItemType::DropMacro => {
                                if force_drop {
                                    ("[DROP MACRO] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::CreateSequence => {
                                ("[SEQUENCE] ", Style::default().fg(Color::LightGreen))
                            }
                            DiffItemType::DropSequence => {
                                if force_drop {
                                    ("[DROP SEQ] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::CreateTrigger => {
                                ("[TRIGGER] ", Style::default().fg(Color::LightMagenta))
                            }
                            DiffItemType::DropTrigger => {
                                if force_drop {
                                    ("[DROP TRG] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                            DiffItemType::AddConstraint => {
                                ("[ADD CONSTRAINT] ", Style::default().fg(Color::Yellow))
                            }
                            DiffItemType::RecreateTable => {
                                if force_drop {
                                    ("[RECREATE TABLE] ", Style::default().fg(Color::Red))
                                } else {
                                    ("[IGNORED RECREATE] ", Style::default().fg(Color::DarkGray))
                                }
                            }
                        };

                        if item.locked {
                            prefix = "[LOCKED] ";
                            style = Style::default()
                                .fg(Color::Red)
                                .add_modifier(Modifier::CROSSED_OUT);
                        } else if !item.selected {
                            style = Style::default().fg(Color::DarkGray);
                        }

                        let line = Line::from(vec![
                            Span::raw(checkbox),
                            Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                            Span::raw(&item.obj_name),
                        ]);

                        ListItem::new(line)
                    })
                    .collect();

                let list = List::new(items)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title("Proposed Changes (Space to toggle, Up/Down to navigate)"),
                    )
                    .highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(">> ");

                f.render_stateful_widget(list, chunks[2], &mut list_state);

                // Render Details Pane (Side-by-Side)
                if let Some(idx) = list_state.selected() {
                    let selected_item = if idx < visible_items.len() {
                        visible_items[idx].1
                    } else {
                        return;
                    };

                    let old_def = selected_item.old_def.as_deref().unwrap_or("");
                    let new_def = selected_item.new_def.as_deref().unwrap_or("");

                    let (left_lines, right_lines) = get_side_by_side_diff(old_def, new_def);

                    let details_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                        .split(chunks[3]);

                    let max_scroll = left_lines.len() as u16;
                    scroll_position = scroll_position.min(max_scroll);

                    let left_pane = Paragraph::new(left_lines)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("Deployed (Online)")
                                .border_style(Style::default().fg(Color::Cyan)),
                        )
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_position, 0));

                    let right_pane = Paragraph::new(right_lines)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("Workspace (Offline)")
                                .border_style(Style::default().fg(Color::Yellow)),
                        )
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_position, 0));

                    f.render_widget(left_pane, details_chunks[0]);
                    f.render_widget(right_pane, details_chunks[1]);
                } else {
                    let p = Paragraph::new("Select an item to view details")
                        .block(Block::default().borders(Borders::ALL).title("Details"));
                    f.render_widget(p, chunks[3]);
                }
            }

            let footer_text = if diff.is_empty() {
                "Press 'Esc' or 'q' to exit"
            } else if inspect_data.is_some() {
                "Press 'Esc' or 'q' to close inspect view"
            } else if show_help {
                "Press 'Esc' or 'q' to close help"
            } else {
                "Press 'Enter' to Apply, 'Space' to toggle, 'b' to switch DB, '?' for Help, 'Esc' or 'q' to Quit"
            };

            let footer = Paragraph::new(Span::styled(footer_text, Style::default().fg(Color::Cyan)))
                .block(
                    Block::default().borders(Borders::ALL).title_bottom(
                        Line::from(vec![
                            Span::styled(
                                " e ",
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                " Explorer / IDE ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])
                        .alignment(Alignment::Right),
                    ),
                );
            f.render_widget(footer, chunks[4]);

            if let Some((obj_name, headers, rows, total_rows)) = inspect_data {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 8,
                    y: f.area().height / 8,
                    width: (f.area().width * 3) / 4,
                    height: (f.area().height * 3) / 4,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let mut lines = Vec::new();
                for (row_idx, row) in rows.iter().enumerate() {
                    lines.push(ratatui::text::Line::from(vec![ratatui::text::Span::styled(
                        format!("--- Row {} ---", row_idx + 1),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )]));
                    for (i, cell) in row.iter().enumerate() {
                        let header = &headers[i];
                        let padding = " ".repeat(20_usize.saturating_sub(header.len()));
                        lines.push(ratatui::text::Line::from(vec![
                            ratatui::text::Span::styled(
                                format!("{}{}", header, padding),
                                Style::default().fg(Color::Cyan),
                            ),
                            ratatui::text::Span::raw(format!(" | {}", cell)),
                        ]));
                    }
                    lines.push(ratatui::text::Line::from(""));
                }

                max_inspect_scroll = lines
                    .len()
                    .saturating_sub((popup_area.height as usize).saturating_sub(2))
                    as u16;
                inspect_scroll = inspect_scroll.min(max_inspect_scroll);

                let paragraph = Paragraph::new(lines)
                    .block(
                        Block::default()
                            .title(format!(
                                " Inspecting: {} (Total Rows: {}) - Scroll: {}/{} ",
                                obj_name, total_rows, inspect_scroll, max_inspect_scroll
                            ))
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .scroll((inspect_scroll, 0));
                f.render_widget(paragraph, popup_area);
            } else if show_help {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 4,
                    y: f.area().height / 4,
                    width: f.area().width / 2,
                    height: f.area().height / 2,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let help_lines = vec![
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [?]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Toggle Help Menu"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [b]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Switch target database catalog"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [p]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Pull dropped item down to local files"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [v]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Toggle visibility of .duckdbignore items"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [d]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Toggle destructive mode (force drop)"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [i]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Inspect a table's data in target"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [c]       ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Copy current status text to duckpack.log"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [Enter]   ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Apply selected changes"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [Space]   ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Toggle selection for an item"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [PgUp/Dn] ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Scroll SQL diff panes"),
                    ]),
                    ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(" [Esc/q]   ", Style::default().fg(Color::Cyan)),
                        ratatui::text::Span::raw("Exit / Close panels"),
                    ]),
                ];

                let help_paragraph = Paragraph::new(help_lines).block(
                    Block::default()
                        .title(" Help Menu ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow)),
                );
                f.render_widget(help_paragraph, popup_area);
            }

            if exit_confirm_modal.is_open {
                let pending_count = diff
                    .items
                    .iter()
                    .filter(|i| i.selected && !i.ignored && (!i.is_destructive() || force_drop))
                    .count();
                let warning_line = if pending_count > 0 {
                    Some(format!(
                        "⚠️ {} pending change(s) have not been applied!",
                        pending_count
                    ))
                } else {
                    None
                };
                current_exit_dialog_rect = exit_confirm_modal.render_with_warning(
                    f,
                    " ⚠️ Exit DuckPack Diff Review ",
                    "Are you sure you want to exit Diff Review?",
                    warning_line.as_deref(),
                );
            }
        })?;

        if draw_only {
            return Ok(TuiAction::Refresh);
        }

        match event::read()? {
            Event::Key(key) => {
                if exit_confirm_modal.is_open {
                    match exit_confirm_modal.handle_key(key) {
                        Some(true) => return Ok(TuiAction::Quit),
                        Some(false) | None => continue,
                    }
                }

                if inspect_data.is_some() {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(TuiAction::CloseInspect);
                        }
                        KeyCode::Up => {
                            inspect_scroll = inspect_scroll.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            inspect_scroll = inspect_scroll.saturating_add(1).min(max_inspect_scroll);
                        }
                        KeyCode::PageUp => {
                            inspect_scroll = inspect_scroll.saturating_sub(10);
                        }
                        KeyCode::PageDown => {
                            inspect_scroll = inspect_scroll.saturating_add(10).min(max_inspect_scroll);
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if show_help {
                            show_help = false;
                        } else {
                            exit_confirm_modal.open();
                        }
                    }
                    KeyCode::Char('?') | KeyCode::Char('h') => {
                        show_help = !show_help;
                    }
                    KeyCode::Char('r') => {
                        return Ok(TuiAction::Refresh);
                    }
                    KeyCode::Char('b') => {
                        return Ok(TuiAction::SwitchCatalog);
                    }
                    KeyCode::Char('e') => {
                        return Ok(TuiAction::Explore);
                    }
                    KeyCode::Char('v') => {
                        show_ignored = !show_ignored;
                    }
                    KeyCode::Char('c') => {
                        return Ok(TuiAction::CopyStatus);
                    }
                    KeyCode::Char('p') => {
                        if let Some(idx) = list_state.selected() {
                            if idx < visible_items.len() {
                                let item = visible_items[idx].1;
                                return Ok(TuiAction::Pull {
                                    obj_name: item.obj_name.clone(),
                                    item_type: item.item_type.clone(),
                                });
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        return Ok(TuiAction::ToggleDestructiveMode);
                    }
                    KeyCode::Char('i') => {
                        if let Some(selected) = list_state.selected() {
                            if selected < visible_items.len() {
                                return Ok(TuiAction::Inspect {
                                    obj_name: visible_items[selected].1.obj_name.clone(),
                                });
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let Some(selected) = list_state.selected() {
                            if selected < visible_items.len().saturating_sub(1) {
                                list_state.select(Some(selected + 1));
                                scroll_position = 0;
                            }
                        }
                    }
                    KeyCode::Up => {
                        if let Some(selected) = list_state.selected() {
                            if selected > 0 {
                                list_state.select(Some(selected - 1));
                                scroll_position = 0;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        scroll_position = scroll_position.saturating_add(5);
                    }
                    KeyCode::PageUp => {
                        scroll_position = scroll_position.saturating_sub(5);
                    }
                    KeyCode::Char(' ') => {
                        if let Some(idx) = list_state.selected() {
                            if idx < visible_items.len() {
                                let real_idx = visible_items[idx].0;
                                if !diff.items[real_idx].locked && !diff.items[real_idx].ignored {
                                    diff.items[real_idx].selected = !diff.items[real_idx].selected;
                                    recompute_locks(&mut diff.items);
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if !diff.is_empty() {
                            return Ok(TuiAction::Apply);
                        }
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse_event) => {
                if exit_confirm_modal.is_open {
                    if mouse_event.kind == event::MouseEventKind::Down(event::MouseButton::Left) {
                        if mouse_event.row == current_exit_dialog_rect.bottom().saturating_sub(2) {
                            let center_x = current_exit_dialog_rect.left()
                                + current_exit_dialog_rect.width / 2;
                            if mouse_event.column >= current_exit_dialog_rect.left()
                                && mouse_event.column < center_x
                            {
                                return Ok(TuiAction::Quit);
                            } else if mouse_event.column >= center_x
                                && mouse_event.column <= current_exit_dialog_rect.right()
                            {
                                exit_confirm_modal.close();
                            }
                        } else if mouse_event.row < current_exit_dialog_rect.top()
                            || mouse_event.row >= current_exit_dialog_rect.bottom()
                            || mouse_event.column < current_exit_dialog_rect.left()
                            || mouse_event.column >= current_exit_dialog_rect.right()
                        {
                            exit_confirm_modal.close();
                        }
                    }
                    continue;
                }

                if inspect_data.is_some() {
                    match mouse_event.kind {
                        event::MouseEventKind::ScrollDown => {
                            inspect_scroll = inspect_scroll.saturating_add(3).min(max_inspect_scroll);
                        }
                        event::MouseEventKind::ScrollUp => {
                            inspect_scroll = inspect_scroll.saturating_sub(3);
                        }
                        _ => {}
                    }
                    continue;
                }

                match mouse_event.kind {
                    event::MouseEventKind::ScrollDown => {
                        scroll_position = scroll_position.saturating_add(3);
                    }
                    event::MouseEventKind::ScrollUp => {
                        scroll_position = scroll_position.saturating_sub(3);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_diff_review_session_headless_actions() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let project_dir = std::env::temp_dir().join(format!("duckpack_tui_test_{}", nanos));
        fs::create_dir_all(project_dir.join("tables")).unwrap();
        fs::write(
            project_dir.join("tables/users.sql"),
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name VARCHAR);",
        )
        .unwrap();

        let db_path = project_dir.join("target.duckdb");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE extra_table (x INTEGER); INSERT INTO extra_table VALUES (10), (20);",
        )
        .unwrap();

        let mut exec = &conn;
        let mut session = DiffReviewSession::new(
            &mut exec,
            &project_dir,
            db_path.to_str().unwrap(),
            None,
            false,
        )
        .unwrap();

        assert_eq!(session.active_catalog, "target");
        assert!(!session.force_drop);
        assert_eq!(session.diff_result.items.len(), 2);

        // 1. Inspect existing table in target DB
        session.inspect_object(&mut exec, "extra_table");
        let (inspected_name, cols, rows, total_rows) = session.inspect_data.as_ref().unwrap();
        assert_eq!(inspected_name, "extra_table");
        assert_eq!(cols, &vec!["x".to_string()]);
        assert_eq!(rows.len(), 2);
        assert_eq!(*total_rows, 2);

        // 2. Toggle destructive mode
        session.toggle_destructive_mode();
        assert!(session.force_drop);
        session.toggle_destructive_mode();
        assert!(!session.force_drop);

        // 3. Pull `extra_table` into local project files
        session
            .pull_object(&mut exec, "extra_table", &DiffItemType::DropTable)
            .unwrap();
        assert!(project_dir.join("tables/extra_table.sql").exists());
        // After pulling `extra_table`, only `CreateTable(users)` remains!
        assert_eq!(session.diff_result.items.len(), 1);
        assert_eq!(session.diff_result.items[0].obj_name, "users");

        // 4. Apply selected changes (`CreateTable(users)`)
        let applied = session.apply_selected(&mut exec).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(session.summary.total_applied_count, 1);
        assert_eq!(session.summary.applied_items, vec!["users".to_string()]);
        assert!(session.diff_result.is_empty());

        drop(conn);
        let _ = fs::remove_dir_all(&project_dir);
    }
}
