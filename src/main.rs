use std::panic;
mod diff;
mod tui;
mod ide;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use duckdb::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug)]
struct DuckpackConfig {
    env: Option<HashMap<String, EnvConfig>>,
}

#[derive(Deserialize, Debug)]
struct EnvConfig {
    db: Option<String>,
}

/// The core CLI argument parser configuration using `clap`.
/// Dictates all available terminal commands for duckpack.
/// 
/// # Educational Note:
/// `clap` is the standard library for CLI parsing in Rust.
/// By deriving `Parser`, we automatically generate help menus (`--help`)
/// and strictly type our terminal arguments.
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Optional environment profile (e.g. dev, prod). Loads .env.<profile>
    #[arg(short, long, global = true)]
    env: Option<String>,
}

/// The supported subcommands for duckpack.
#[derive(Subcommand)]
enum Commands {
    /// Initialize a new DuckDB migration project in the target directory
    Init {
        /// The path where the project should be initialized
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,
    },
    /// Validate the DB project syntax using an in-memory Shadow DB
    Build {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,
    },
    /// Apply the DB project to the target DuckDB database
    Apply {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,

        /// The connection string or path to the target DuckDB database
        #[arg(short, long, default_value = "local.duckdb")]
        db: String,
        /// Allow destructive drops (tables/columns)
        #[arg(long)]
        force_drop: bool,
        /// Apply automatically without showing the TUI
        #[arg(long)]
        auto_approve: bool,
        /// Dry run mode: output migration SQL script to this file instead of applying
        #[arg(long)]
        out: Option<String>,
        /// Quack authentication token
        #[arg(long)]
        quack_token: Option<String>,
        /// Disable SSL/TLS verification for Quack connection (plain HTTP)
        #[arg(long, visible_alias = "insecure", alias = "quack-insecure")]
        quack_disable_ssl: bool,
    },
    /// Compile the DB project into a standalone .duckpack artifact
    Compile {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,
        /// The path to output the compiled .duckpack artifact
        #[arg(short, long)]
        out: String,
    },
    /// Deploy a DB project to a remote server via SCP/SSH
    Deploy {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,
        /// The remote server SSH target (e.g. user@10.0.0.5)
        #[arg(short, long)]
        remote: String,
        /// The SSH port to use for remote deployment
        #[arg(short = 'P', long, default_value = "22")]
        port: String,
        /// The connection string or path to the target DuckDB database on the remote server
        #[arg(short, long, default_value = "local.duckdb")]
        db: String,
        /// Automatically install the duckpack CLI binary on the remote server
        #[arg(long)]
        auto_install: bool,
    },
    /// Explore a live DuckDB database in the built-in IDE
    Explore {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,

        /// The connection string or path to the target DuckDB database
        #[arg(short, long, default_value = "local.duckdb")]
        db: String,
        /// The remote server SSH target (e.g. user@10.0.0.5)
        #[arg(long)]
        remote: Option<String>,
        /// The SSH port to use for remote deployment
        #[arg(short = 'P', long, default_value = "22")]
        port: String,
        /// Automatically install the duckpack CLI binary on the remote server
        #[arg(long)]
        auto_install: bool,
        /// Quack authentication token
        #[arg(long)]
        quack_token: Option<String>,
        /// Disable SSL/TLS verification for Quack connection (plain HTTP)
        #[arg(long, visible_alias = "insecure", alias = "quack-insecure")]
        quack_disable_ssl: bool,
    },
}

/// Determines if the provided database string refers to a remote motherduck instance.
fn is_remote(db: &str) -> bool {
    db.starts_with("md:") || db.starts_with("motherduck:")
}

/// Determines if the provided quack connection URI targets the 0.0.0.0 local unspecified address.
fn is_quack_zero_host(db: &str) -> bool {
    let stripped = db.strip_prefix("quack://").or_else(|| db.strip_prefix("quack:")).unwrap_or(db);
    stripped.starts_with("0.0.0.0") || stripped.starts_with("[::]")
}

/// Determines if SSL should be disabled for a Quack connection.
/// Returns true if explicitly flagged, if DUCKDB_QUACK_DISABLE_SSL is set, or if connecting to 0.0.0.0.
fn should_disable_quack_ssl(db: &str, flag_disable_ssl: bool) -> bool {
    flag_disable_ssl
        || std::env::var("DUCKDB_QUACK_DISABLE_SSL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
        || is_quack_zero_host(db)
}

/// Builds the SQL ATTACH statement for a Quack database.
fn build_quack_attach_sql(db: &str, disable_ssl: bool) -> String {
    let safe_db = db.replace("'", "''");
    if disable_ssl {
        format!("ATTACH '{}' AS target_db (DISABLE_SSL true);", safe_db)
    } else {
        format!("ATTACH '{}' AS target_db;", safe_db)
    }
}

/// Connects to the target DuckDB database (local, motherduck remote, or quack remote).
fn connect_target_db(
    db: &str,
    quack_token: Option<&str>,
    quack_disable_ssl: bool,
) -> Result<Connection> {
    if db.starts_with("quack:") {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        conn.execute_batch("INSTALL quack; LOAD quack;")
            .with_context(|| "Failed to install/load quack extension")?;

        let token = quack_token
            .map(|s| s.to_string())
            .or_else(|| std::env::var("DUCKDB_QUACK_TOKEN").ok());

        if let Some(token) = token {
            let safe_token = token.replace("'", "''");
            conn.execute(&format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token), [])
                .with_context(|| "Failed to set quack auth secret")?;
        }

        let disable_ssl = should_disable_quack_ssl(db, quack_disable_ssl);
        let attach_sql = build_quack_attach_sql(db, disable_ssl);

        conn.execute(&attach_sql, [])
            .with_context(|| "Failed to attach remote database")?;
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
        Ok(conn)
    } else if is_remote(db) {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        let safe_db = db.replace("'", "''");
        conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), [])
            .with_context(|| "Failed to attach remote database")?;
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
        Ok(conn)
    } else {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for local ATTACH")?;
        let safe_db = db.replace("'", "''");
        conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), [])
            .with_context(|| "Failed to attach local database")?;
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
        Ok(conn)
    }
}

/// The main entry point for the DuckPack CLI.
/// Parses the arguments and delegates to the appropriate command handler.
fn main() -> Result<()> {
    panic::set_hook(Box::new(|info| { let bt = std::backtrace::Backtrace::force_capture(); let _ = std::fs::write("panic.log", format!("Panic: {:?}\n\n{}", info, bt)); }));
    let mut cli = Cli::parse();

    let project_dir = match &cli.command {
        Commands::Init { project_dir } => project_dir,
        Commands::Build { project_dir } => project_dir,
        Commands::Apply { project_dir, .. } => project_dir,
        Commands::Compile { project_dir, .. } => project_dir,
        Commands::Deploy { project_dir, .. } => project_dir,
        Commands::Explore { project_dir, .. } => project_dir,
    };

    // Load .env file into process environment if it exists
    let env_file = if let Some(e) = &cli.env {
        format!(".env.{}", e)
    } else {
        ".env".to_string()
    };
    dotenvy::from_path(project_dir.join(env_file)).ok();

    // Check for duckpack.toml
    let toml_path = project_dir.join("duckpack.toml");
    if toml_path.exists() {
        if let Ok(content) = fs::read_to_string(&toml_path) {
            if let Ok(config) = toml::from_str::<DuckpackConfig>(&content) {
                if let Some(cli_env) = &cli.env {
                    if let Some(envs) = config.env {
                        if let Some(env_config) = envs.get(cli_env) {
                            if let Some(toml_db) = &env_config.db {
                                // Override the db argument for applicable commands
                                match &mut cli.command {
                                    Commands::Apply { db, .. } |
                                    Commands::Deploy { db, .. } |
                                    Commands::Explore { db, .. } => {
                                        *db = toml_db.clone();
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    match &cli.command {
        Commands::Init { project_dir } => {
            println!("Initializing DuckDB project in {}", project_dir.display());
            init_project(project_dir)?;
            println!("Project initialized successfully!");
        }
        Commands::Build { project_dir } => {
            println!("Building project in {}", project_dir.display());
            let _ = build_shadow_db(project_dir)?;
            println!("Build successful! All SQL syntax is valid.");
        }
        Commands::Apply { project_dir, db, force_drop, auto_approve, out, quack_token, quack_disable_ssl } => {
            if !project_dir.exists() {
                eprintln!("Project directory {} does not exist. Run init first.", project_dir.display());
                return Ok(());
            }

            let mut terminal = if !*auto_approve && out.is_none() {
                ratatui::crossterm::terminal::enable_raw_mode()?;
                let mut stdout = std::io::stdout();
                ratatui::crossterm::execute!(stdout, ratatui::crossterm::terminal::EnterAlternateScreen, ratatui::crossterm::event::EnableMouseCapture)?;
                let backend = ratatui::backend::CrosstermBackend::new(stdout);
                Some(ratatui::Terminal::new(backend)?)
            } else {
                None
            };

            let mut status_msg = String::from("Ready. Press 'Enter' to Apply, 'r' to Refresh.");
            let mut current_force_drop = *force_drop;
            let mut inspect_data: Option<(String, Vec<String>, Vec<Vec<String>>, i64)> = None;

            loop {
                let shadow_conn = build_shadow_db(project_dir)?;
                
                let target_conn = connect_target_db(db, quack_token.as_deref(), *quack_disable_ssl)?;
                
                let shadow_schemas = diff::get_schemas(&shadow_conn)?;
                let target_schemas = diff::get_schemas(&target_conn)?;
                let shadow_schema = diff::get_schema(&shadow_conn)?;
                let target_schema = diff::get_schema(&target_conn)?;
                let shadow_views = diff::get_views(&shadow_conn)?;
                let target_views = diff::get_views(&target_conn)?;
                let shadow_macros = diff::get_macros(&shadow_conn)?;
                let target_macros = diff::get_macros(&target_conn)?;

                let mut ignore_list = Vec::new();
                if let Ok(ignore_content) = std::fs::read_to_string(project_dir.join(".duckdbignore")) {
                    for line in ignore_content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with('#') {
                            ignore_list.push(trimmed.to_string());
                        }
                    }
                }

                let mut diff_result = diff::compute_diff(
                    &shadow_schemas,
                    &target_schemas,
                    &shadow_schema, 
                    &target_schema, 
                    &shadow_views, 
                    &target_views, 
                    &shadow_macros, 
                    &target_macros, 
                    project_dir,
                    &ignore_list
                );

                if let Some(out_file) = out {
                    let mut script = String::new();
                    for item in &diff_result.items {
                        let is_drop = matches!(item.item_type, diff::DiffItemType::DropTable | diff::DiffItemType::DropColumn | diff::DiffItemType::DropView | diff::DiffItemType::DropMacro);
                        if is_drop && !*force_drop {
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
                    if let Err(e) = std::fs::write(out_file, script) {
                        eprintln!("Error writing dry-run script: {}", e);
                    } else {
                        println!("Dry-run script successfully written to {}", out_file);
                    }
                    break;
                }

                if *auto_approve {
                    let remote = is_remote(db);
                    let apply_success = apply_diff(&target_conn, &diff_result, current_force_drop, remote, project_dir);
                    if apply_success.is_ok() {
                        println!("Changes applied automatically!");
                    } else {
                        eprintln!("Error applying schema: {:?}", apply_success.err());
                    }
                    break;
                }

                if let Some(term) = &mut terminal {
                    match tui::draw_and_handle_events(term, &mut diff_result, current_force_drop, &status_msg, false, &inspect_data, &db)? {
                        tui::TuiAction::Quit => {
                            break;
                        }
                    tui::TuiAction::CopyStatus => {
                        let _ = std::fs::write("duckpack.log", &status_msg);
                        status_msg = "Status text copied to duckpack.log!".to_string();
                        continue;
                    }
                    tui::TuiAction::Refresh => {
                        status_msg = "Refreshed schema from disk.".to_string();
                        continue;
                    }
                    tui::TuiAction::ToggleDestructiveMode => {
                        current_force_drop = !current_force_drop;
                        status_msg = if current_force_drop {
                            "Destructive Mode ENABLED. Drops will be executed.".to_string()
                        } else {
                            "Destructive Mode DISABLED. Drops will be ignored.".to_string()
                        };
                        continue;
                    }
                    tui::TuiAction::Inspect { obj_name } => {
                        let safe_obj = obj_name.replace("'", "''");
                        
                        let mut cols = Vec::new();
                        if let Ok(mut pragma_stmt) = target_conn.prepare(&format!("PRAGMA table_info('{}')", safe_obj)) {
                            if let Ok(mut pragma_rows) = pragma_stmt.query([]) {
                                while let Ok(Some(row)) = pragma_rows.next() {
                                    if let Ok(col_name) = row.get::<_, String>(1) {
                                        cols.push(col_name);
                                    }
                                }
                            }
                        }

                        if cols.is_empty() {
                            status_msg = format!("Cannot inspect '{}': Does not exist in target database yet.", obj_name);
                        } else {
                            let mut row_count = 0;
                            if let Ok(mut count_stmt) = target_conn.prepare(&format!("SELECT count(*) FROM \"{}\"", safe_obj)) {
                                if let Ok(mut count_rows) = count_stmt.query([]) {
                                    if let Ok(Some(row)) = count_rows.next() {
                                        row_count = row.get::<_, i64>(0).unwrap_or(0);
                                    }
                                }
                            }

                            let cast_cols = cols.iter().map(|c| format!("CAST(\"{}\" AS VARCHAR)", c)).collect::<Vec<_>>().join(", ");
                            if let Ok(mut stmt) = target_conn.prepare(&format!("SELECT {} FROM \"{}\" LIMIT 5", cast_cols, safe_obj)) {
                                if let Ok(mut db_rows) = stmt.query([]) {
                                    let mut rows = Vec::new();
                                    while let Ok(Some(row)) = db_rows.next() {
                                        let mut str_row = Vec::new();
                                        for i in 0..cols.len() {
                                            let val: String = row.get(i).unwrap_or_else(|_| "NULL".to_string());
                                            str_row.push(val);
                                        }
                                        rows.push(str_row);
                                    }
                                    inspect_data = Some((obj_name, cols, rows, row_count));
                                }
                            }
                        }
                        continue;
                    }
                    tui::TuiAction::CloseInspect => {
                        inspect_data = None;
                        continue;
                    }
                    tui::TuiAction::Explore => {
                        if let Some(term) = &mut terminal {
                            ide::run_ide_loop(term, &target_conn, &project_dir, &db)?;
                            term.clear()?;
                        }
                        continue;
                    }
                    tui::TuiAction::Apply => {
                        status_msg = "Applying changes...".to_string();
                        if let Some(term) = &mut terminal {
                            let _ = tui::draw_and_handle_events(term, &mut diff_result, current_force_drop, &status_msg, true, &inspect_data, &db);
                        }

                        let remote = is_remote(db);
                        if !remote {
                            let db_path = PathBuf::from(db);
                            let mut b_path = db_path.clone();
                            b_path.set_extension("duckdb.bak");
                            if db_path.exists() {
                                let _ = fs::copy(&db_path, &b_path);
                            }
                            
                            let apply_success = apply_diff(&target_conn, &diff_result, current_force_drop, remote, project_dir);
                            
                            if apply_success.is_ok() {
                                status_msg = "Changes applied successfully!".to_string();
                                if b_path.exists() {
                                    fs::remove_file(&b_path).ok();
                                }
                            } else {
                                status_msg = format!("Error applying schema: {:?}", apply_success.err());
                                drop(target_conn); 
                                if b_path.exists() {
                                    let _ = fs::copy(&b_path, &db_path);
                                    fs::remove_file(&b_path).ok();
                                }
                            }
                        } else {
                            let apply_success = apply_diff(&target_conn, &diff_result, current_force_drop, remote, project_dir);
                            if apply_success.is_ok() {
                                status_msg = "Changes applied successfully to remote database!".to_string();
                            } else {
                                status_msg = format!("Error applying schema to remote database: {:?}", apply_success.err());
                            }
                        }
                    }

                    tui::TuiAction::Pull { obj_name, item_type } => {
                        let sql = match item_type {
                            diff::DiffItemType::DropTable => {
                                if let Some(target) = target_schema.get(&obj_name) {
                                    Some(target.create_sql.clone())
                                } else { None }
                            }
                            diff::DiffItemType::DropView => target_views.get(&obj_name).cloned(),
                            diff::DiffItemType::DropMacro => target_macros.get(&obj_name).cloned(),
                            _ => None,
                        };

                        if let Some(sql) = sql {
                            let mut path = project_dir.clone();
                            match item_type {
                                diff::DiffItemType::DropTable => path.push("tables"),
                                diff::DiffItemType::DropView => path.push("views"),
                                diff::DiffItemType::DropMacro => path.push("macros"),
                                _ => {}
                            }
                            path.push(format!("{}.sql", obj_name));
                            if let Err(e) = std::fs::write(&path, sql) {
                                status_msg = format!("Error pulling '{}': {}", obj_name, e);
                            } else {
                                status_msg = format!("Successfully pulled '{}' back into local project!", obj_name);
                            }
                        } else {
                            status_msg = format!("Cannot pull '{}': Definition not found in remote DB.", obj_name);
                        }
                    }
                }
                }
            }

            if !*auto_approve {
                if let Some(term) = &mut terminal {
                    ratatui::crossterm::terminal::disable_raw_mode()?;
                    ratatui::crossterm::execute!(std::io::stdout(), ratatui::crossterm::terminal::LeaveAlternateScreen, ratatui::crossterm::event::DisableMouseCapture)?;
                    term.show_cursor()?;
                }
            }
        }
        Commands::Compile { project_dir, out } => {
            println!("Compiling project {} into DuckPack: {}", project_dir.display(), out);
            let _shadow_conn = build_shadow_db(project_dir)?;
            
            // DuckPack format: A simple duckdb file.
            let _ = std::fs::remove_file(&out); // Remove if exists
            let out_conn = Connection::open(&out).with_context(|| "Failed to create .duckpack file")?;
            println!("Packaging tables, views, and macros...");
            
            let schemas_dir = project_dir.join("schemas");
            if schemas_dir.exists() {
                execute_directory(&out_conn, &schemas_dir)?;
            }
            let tables_dir = project_dir.join("tables");
            if tables_dir.exists() {
                execute_directory(&out_conn, &tables_dir)?;
            }
            let views_dir = project_dir.join("views");
            if views_dir.exists() {
                execute_directory(&out_conn, &views_dir)?;
            }
            let macros_dir = project_dir.join("macros");
            if macros_dir.exists() {
                execute_directory(&out_conn, &macros_dir)?;
            }
            println!("Compilation complete! Artifact: {}", out);
        }
        Commands::Explore { project_dir, db, remote, port, auto_install, quack_token, quack_disable_ssl } => {
            if let Some(r) = remote {
                let mut remote_bin = "duckpack".to_string();
                let mut remote_tmp_bin = None;
                let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                
                if *auto_install {
                    println!("Auto-installing DuckPack companion binary for exploration...");
                    let current_exe = std::env::current_exe().with_context(|| "Failed to get current executable path")?;
                    let tmp_bin = format!("/tmp/duckpack_bin_{}", ts);
                    
                    let scp_bin_status = std::process::Command::new("scp")
                        .arg("-P")
                        .arg(port.to_string())
                        .arg(&current_exe)
                        .arg(format!("{}:{}", r, tmp_bin))
                        .status()?;
                        
                    if !scp_bin_status.success() {
                        anyhow::bail!("Failed to SCP binary to remote server.");
                    }
                    
                    let chmod_status = std::process::Command::new("ssh")
                        .arg("-p")
                        .arg(port.to_string())
                        .arg(r)
                        .arg(format!("chmod +x {}", tmp_bin))
                        .status()?;
                        
                    if !chmod_status.success() {
                        anyhow::bail!("Failed to make binary executable on remote server.");
                    }
                    
                    remote_bin = tmp_bin.clone();
                    remote_tmp_bin = Some(tmp_bin);
                }

                let remote_tmp_proj = format!("/tmp/duckpack_explore_{}", ts);
                let _ = std::process::Command::new("ssh")
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(r)
                    .arg(format!("mkdir -p {}/queries", remote_tmp_proj))
                    .status();
                
                println!("Launching remote TUI over SSH...");
                let mut remote_cmd = format!("{} explore --project-dir {} --db {}", remote_bin, remote_tmp_proj, db);
                if let Some(token) = quack_token {
                    remote_cmd.push_str(&format!(" --quack-token {}", token));
                }
                if *quack_disable_ssl {
                    remote_cmd.push_str(" --quack-disable-ssl");
                }
                let ssh_status = std::process::Command::new("ssh")
                    .arg("-t") // Force pseudo-terminal for TUI
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(r)
                    .arg(remote_cmd)
                    .status()?;

                if !ssh_status.success() {
                    println!("Remote exploration session ended with error.");
                }

                println!("Cleaning up remote artifacts...");
                let mut cleanup_cmd = format!("rm -rf {}", remote_tmp_proj);
                if let Some(tmp_bin) = remote_tmp_bin {
                    cleanup_cmd.push_str(&format!(" {}", tmp_bin));
                }
                let _ = std::process::Command::new("ssh")
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(r)
                    .arg(cleanup_cmd)
                    .status();

                return Ok(());
            }

            let mut project_dir = project_dir.clone();
            if project_dir == std::path::PathBuf::from(".") && !db.starts_with("quack:") && !is_remote(&db) {
                let db_path = std::path::PathBuf::from(&db);
                if let Some(parent) = db_path.parent() {
                    if parent != std::path::Path::new("") {
                        project_dir = parent.to_path_buf();
                    }
                }
            }

            if !project_dir.exists() {
                eprintln!("Project directory {} does not exist. Run init first.", project_dir.display());
                return Ok(());
            }

            ratatui::crossterm::terminal::enable_raw_mode()?;
            let mut stdout = std::io::stdout();
            ratatui::crossterm::execute!(stdout, ratatui::crossterm::terminal::EnterAlternateScreen, ratatui::crossterm::event::EnableMouseCapture)?;
            let backend = ratatui::backend::CrosstermBackend::new(stdout);
            let mut terminal = ratatui::Terminal::new(backend)?;

            let target_conn = connect_target_db(&db, quack_token.as_deref(), *quack_disable_ssl)?;

            ide::run_ide_loop(&mut terminal, &target_conn, &project_dir, &db)?;

            ratatui::crossterm::terminal::disable_raw_mode()?;
            ratatui::crossterm::execute!(
                terminal.backend_mut(),
                ratatui::crossterm::terminal::LeaveAlternateScreen,
                ratatui::crossterm::event::DisableMouseCapture
            )?;
        }
        Commands::Deploy { project_dir, remote, db, port, auto_install } => {
            println!("Deploying {} to {} at {}", project_dir.display(), remote, db);
            
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            let tmp_pack = format!("/tmp/duckpack_deploy_{}.duckpack", ts);
            
            println!("1. Compiling local DuckPack artifact...");
            let out_conn = Connection::open(&tmp_pack).with_context(|| "Failed to create temp .duckpack file")?;
            let schemas_dir = project_dir.join("schemas");
            if schemas_dir.exists() { execute_directory(&out_conn, &schemas_dir)?; }
            let tables_dir = project_dir.join("tables");
            if tables_dir.exists() { execute_directory(&out_conn, &tables_dir)?; }
            let views_dir = project_dir.join("views");
            if views_dir.exists() { execute_directory(&out_conn, &views_dir)?; }
            let macros_dir = project_dir.join("macros");
            if macros_dir.exists() { execute_directory(&out_conn, &macros_dir)?; }
            out_conn.close().map_err(|e| anyhow::anyhow!("Failed to close DuckPack: {:?}", e.1))?;
            
            let remote_tmp_pack = format!("/tmp/deploy_{}.duckpack", ts);
            
            println!("2. Transferring artifact to remote server (scp)...");
            let scp_status = std::process::Command::new("scp")
                .arg("-P")
                .arg(port.to_string())
                .arg(&tmp_pack)
                .arg(format!("{}:{}", remote, remote_tmp_pack))
                .status()?;
            
            if !scp_status.success() {
                anyhow::bail!("Failed to SCP artifact to remote server.");
            }
            
            let mut remote_bin = "duckpack".to_string();
            let mut remote_tmp_bin = None;
            
            if *auto_install {
                println!("2.5. Auto-installing DuckPack companion binary...");
                let current_exe = std::env::current_exe().with_context(|| "Failed to get current executable path")?;
                let tmp_bin = format!("/tmp/duckpack_bin_{}", ts);
                
                let scp_bin_status = std::process::Command::new("scp")
                    .arg("-P")
                    .arg(port.to_string())
                    .arg(&current_exe)
                    .arg(format!("{}:{}", remote, tmp_bin))
                    .status()?;
                    
                if !scp_bin_status.success() {
                    anyhow::bail!("Failed to SCP binary to remote server.");
                }
                
                // Make the binary executable
                let chmod_status = std::process::Command::new("ssh")
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(remote)
                    .arg(format!("chmod +x {}", tmp_bin))
                    .status()?;
                    
                if !chmod_status.success() {
                    anyhow::bail!("Failed to make binary executable on remote server.");
                }
                
                remote_bin = tmp_bin.clone();
                remote_tmp_bin = Some(tmp_bin);
            }
            
            println!("3. Executing companion CLI via SSH...");
            let ssh_status = std::process::Command::new("ssh")
                .arg("-p")
                .arg(port.to_string())
                .arg(remote)
                .arg(format!("{} apply --project-dir {} --db {} --auto-approve", remote_bin, remote_tmp_pack, db))
                .status()?;
                
            if !ssh_status.success() {
                println!("Deployment failed during remote execution.");
            } else {
                println!("Deployment successful!");
            }
            
            println!("4. Cleaning up temporary artifacts...");
            let _ = std::fs::remove_file(&tmp_pack);
            
            let mut cleanup_cmd = format!("rm -f {}", remote_tmp_pack);
            if let Some(tmp_bin) = remote_tmp_bin {
                cleanup_cmd.push_str(&format!(" {}", tmp_bin));
            }
            
            let _ = std::process::Command::new("ssh")
                .arg("-p")
                .arg(port.to_string())
                .arg(remote)
                .arg(cleanup_cmd)
                .status();
        }
    }

    Ok(())
}

/// Executes the computed schema differences against the target database.
/// If running against a remote database, it encapsulates the entire deployment inside a single transaction
/// so it can rollback gracefully upon failure.
fn apply_diff(conn: &Connection, diff: &diff::DiffResult, force_drop: bool, is_remote: bool, project_dir: &PathBuf) -> Result<()> {
    if is_remote {
        conn.execute_batch("BEGIN TRANSACTION;")?;
    }

    let result = (|| -> Result<()> {
        let pre_deploy_dir = project_dir.join("scripts").join("pre-deploy");
        if pre_deploy_dir.exists() {
            execute_directory(conn, &pre_deploy_dir)?;
        }

        let mut views_to_apply = Vec::new();
        let mut drops_to_apply = Vec::new();
        let mut drop_schemas = Vec::new();

        // 1. Create schemas first
        for item in &diff.items {
            if !item.selected { continue; }
            if item.item_type == diff::DiffItemType::CreateSchema {
                conn.execute_batch(&item.sql)?;
            }
        }

        let mut structure_to_apply = Vec::new();
        for item in &diff.items {
            if !item.selected { continue; }
            match item.item_type {
                diff::DiffItemType::CreateTable | diff::DiffItemType::AlterTable | diff::DiffItemType::CreateMacro | diff::DiffItemType::RenameTable | diff::DiffItemType::RenameColumn => {
                    structure_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::DropTable | diff::DiffItemType::DropColumn | diff::DiffItemType::DropView | diff::DiffItemType::DropMacro => {
                    if force_drop {
                        drops_to_apply.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::DropSchema => {
                    if force_drop {
                        drop_schemas.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::CreateView => {
                    views_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::CreateSchema => {} // handled above
            }
        }

        if !structure_to_apply.is_empty() {
            execute_sqls_with_retry(conn, structure_to_apply)?;
        }


        // 3. Views
        if !views_to_apply.is_empty() {
            execute_sqls_with_retry(conn, views_to_apply)?;
        }

        // 4. Drops
        for drop_sql in drops_to_apply {
            let _ = conn.execute_batch(&drop_sql); // Ignore errors if already dropped by cascade
        }
        for drop_sql in drop_schemas {
            let _ = conn.execute_batch(&drop_sql);
        }

        let post_deploy_dir = project_dir.join("scripts").join("post-deploy");
        if post_deploy_dir.exists() {
            execute_directory(conn, &post_deploy_dir)?;
        }

        Ok(())
    })();

    if is_remote {
        if result.is_ok() {
            conn.execute_batch("COMMIT;")?;
        } else {
            conn.execute_batch("ROLLBACK;")?;
        }
    }
    result
}

/// Scaffolds a new declarative database project by generating the standard directory structure,
/// configuration files (`duck-project.toml`), environment templates, and documentation.
fn init_project(project_dir: &PathBuf) -> Result<()> {
    let dirs = ["schemas", "tables", "views", "macros", "queries", "seeds", "scripts/pre-deploy", "scripts/post-deploy"];
    for dir in dirs.iter() {
        let path = project_dir.join(dir);
        if !path.exists() {
            fs::create_dir_all(&path).with_context(|| format!("Failed to create directory {}", path.display()))?;
            println!("Created directory: {}", path.display());
        }
    }

    let config_path = project_dir.join("duck-project.toml");
    if !config_path.exists() {
        let default_config = r#"[project]
name = "my_duckdb_project"
version = "0.1.0"
description = "A declarative DuckDB database project"

[target]
default_db = "local.duckdb"
"#;
        fs::write(&config_path, default_config).with_context(|| "Failed to write duck-project.toml")?;
        println!("Created config file: {}", config_path.display());
    }

    let readme_path = project_dir.join("README.md");
    if !readme_path.exists() {
        let readme_content = r#"# DuckDB Database Project

Welcome to your declarative DuckDB project! This directory contains the desired state of your database schema.

## Project Structure

- `schemas/`: Contains `CREATE SCHEMA` statements. Schemas deleted from this folder will be dropped **CASCADE** on the target DB!
- `tables/`: Contains `CREATE TABLE` statements (one per file).
- `views/`: Contains `CREATE VIEW` statements (engine auto-forces `OR REPLACE`).
- `macros/`: Contains `CREATE MACRO` statements.
- `queries/`: Save your ad-hoc `.sql` queries and scratchpad files here.
- `seeds/`: Contains static CSVs or seed scripts.
- `scripts/`: Contains `pre-deploy` and `post-deploy` scripts.
- `duck-project.toml`: Project configuration and targets.
- `.env`: Environment variables for template interpolation. Write `${VAR_NAME}` in your SQL, and the engine will dynamically replace it!
- `.duckdbignore`: Specifies tables or views to ignore during diffing (wildcards supported).

## CLI Commands

### 1. Build & Validate
Validates your project syntax and verifies dependencies by compiling it against an in-memory Shadow DB.

```bash
duckpack build --project-dir .
```

### 2. Apply 
Computes the diff against the target database and opens an interactive TUI to review and selectively apply the changes.

```bash
duckpack apply --project-dir . --db local.duckdb
```

**Options:**
- `--force-drop`: By default, the engine operates in 'Safe Mode' and ignores deleted tables/columns to prevent data loss. Pass this flag to allow destructive `DROP TABLE` and `DROP COLUMN` commands.
- `--auto-approve`: Apply changes without interactive confirmation.
- `--out <FILE>`: Dry-run mode. Generates the migration SQL script and saves it to the specified file without applying it to the database.
- `--env <PROFILE>`: Global flag to load `.env.<PROFILE>` instead of the default `.env`.

**TUI Controls:**
- `Up/Down`: Navigate the list of proposed changes.
- `Space`: Toggle selection (checked items will be applied, unchecked items will be ignored).
- `Enter`: Apply the selected changes transactionally.
- `e`: Open the IDE Explorer to run queries.
- `c`: Copy the current status text to `duckpack.log`.
- `Esc / q`: Cancel the operation.

### 3. Explore
Launch the fully featured built-in IDE to query and explore your DuckDB database safely.

```bash
duckpack explore --project-dir . --db local.duckdb
```

**IDE Features:**
- `Ctrl+E`: Execute query and see formatted results (supports horizontal scrolling via Left/Right arrows).
- `Ctrl+O`: Export the results of the active query to a CSV file in the `queries/` directory.
- `Ctrl+Space`: Autocomplete tables and column names.
- `Ctrl+F`: Format SQL code.
- `Ctrl+S`: Save current scratchpad query.
- `Ctrl+T`: Open a new blank query tab.
- `Ctrl+W`: Close the active tab.
- `Ctrl+N` / `Ctrl+P`: Navigate to the next/previous tab.
- **Syntax Validation:** Highlights syntax errors in red automatically.
- **Query History**: Automatically logs successfully executed queries and timestamps to `queries/.history.sql`.
- **Path Resolution:** Automatically infers `--project-dir` from the database path if omitted.
"#;
        fs::write(&readme_path, readme_content).with_context(|| "Failed to write README.md")?;
        println!("Created documentation file: {}", readme_path.display());
    }

    let env_path = project_dir.join(".env");
    if !env_path.exists() {
        let env_content = r#"# Define environment variables here to interpolate into your SQL files.
# Example: 
# ENV=dev
"#;
        fs::write(&env_path, env_content).with_context(|| "Failed to write .env")?;
        println!("Created environment file: {}", env_path.display());
    }

    let ignore_path = project_dir.join(".duckdbignore");
    if !ignore_path.exists() {
        let ignore_content = r#"# Specify tables or views to ignore during diffing (wildcards supported).
# Example:
# temp_*
# *_backup
"#;
        fs::write(&ignore_path, ignore_content).with_context(|| "Failed to write .duckdbignore")?;
        println!("Created ignore file: {}", ignore_path.display());
    }

    let gitignore_path = project_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = ".env*\n*.duckdb\n*.history.sql\nREADME.md\n*.csv\n*.duckpack\nduckpack.log\nmigration.sql\n";
        fs::write(&gitignore_path, gitignore_content).with_context(|| "Failed to write .gitignore")?;
        println!("Created .gitignore file: {}", gitignore_path.display());
    }

    Ok(())
}

/// Compiles all local `.sql` files into an in-memory DuckDB connection.
/// This acts as the "Desired State" or "Shadow DB" used for validation and diff generation.
fn build_shadow_db(project_dir: &PathBuf) -> Result<Connection> {
    if project_dir.is_file() && project_dir.extension().unwrap_or_default() == "duckpack" {
        return Connection::open(project_dir).with_context(|| "Failed to open .duckpack artifact");
    }

    let conn = Connection::open_in_memory().with_context(|| "Failed to create Shadow DB")?;
    
    let schemas_dir = project_dir.join("schemas");
    if schemas_dir.exists() {
        execute_directory(&conn, &schemas_dir)?;
    }

    let macros_dir = project_dir.join("macros");
    if macros_dir.exists() {
        execute_directory(&conn, &macros_dir)?;
    }

    let tables_dir = project_dir.join("tables");
    if tables_dir.exists() {
        execute_directory(&conn, &tables_dir)?;
    }

    let views_dir = project_dir.join("views");
    if views_dir.exists() {
        execute_directory(&conn, &views_dir)?;
    }

    Ok(conn)
}

/// A helper utility to iterate over a directory of `.sql` files and execute them against a connection.
/// This uses a retry loop because tables/views often depend on each other (e.g. foreign keys or nested views).
/// If one fails, it is pushed to the back of the queue. The loop terminates once all succeed, or if no progress is made.
fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    let mut pending_sqls = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    
    for entry in entries {
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            pending_sqls.push(diff::read_sql_with_env(&path)?);
        }
    }
    
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match conn.execute_batch(&sql) {
                Ok(_) => {
                    progress_made = true;
                }
                Err(e) => {
                    last_error = Some(anyhow::anyhow!("Failed: {}", e));
                    remaining.push(sql);
                }
            }
        }
        pending_sqls = remaining;
    }

    if !pending_sqls.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context(format!("Failed to compile SQL files in {} due to unresolvable dependencies or syntax errors", dir.display()));
        }
    }

    Ok(())
}

/// A specialized execution engine for arbitrary SQLs.
/// Because objects often depend on each other, this algorithm continuously loops over the list of statements
/// and attempts to execute them. If one fails, it is pushed to the back of the queue.
/// The loop terminates once all succeed, or if it completes a full pass without making any progress (circular dependency).
fn execute_sqls_with_retry(conn: &Connection, mut pending_sqls: Vec<String>) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match conn.execute_batch(&sql) {
                Ok(_) => {
                    progress_made = true;
                }
                Err(e) => {
                    last_error = Some(anyhow::anyhow!("Failed: {}", e));
                    remaining.push(sql);
                }
            }
        }
        pending_sqls = remaining;
    }

    if !pending_sqls.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context("Failed to compile SQLs due to unresolvable dependencies or syntax errors");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_remote() {
        assert_eq!(is_remote("md:my_db"), true);
        assert_eq!(is_remote("motherduck:"), true);
        assert_eq!(is_remote("local.db"), false);
        assert_eq!(is_remote("/var/lib/duckdb/prod.db"), false);
    }

    #[test]
    fn test_is_quack_zero_host() {
        assert!(is_quack_zero_host("quack:0.0.0.0:9494"));
        assert!(is_quack_zero_host("quack://0.0.0.0:9494"));
        assert!(is_quack_zero_host("quack:[::]:9494"));
        assert!(!is_quack_zero_host("quack:localhost:9494"));
        assert!(!is_quack_zero_host("quack:127.0.0.1:9494"));
        assert!(!is_quack_zero_host("quack:remote.example.com"));
    }

    #[test]
    fn test_build_quack_attach_sql() {
        assert_eq!(
            build_quack_attach_sql("quack:0.0.0.0:9494", true),
            "ATTACH 'quack:0.0.0.0:9494' AS target_db (DISABLE_SSL true);"
        );
        assert_eq!(
            build_quack_attach_sql("quack:remote.example.com", false),
            "ATTACH 'quack:remote.example.com' AS target_db;"
        );
    }

    #[test]
    fn test_should_disable_quack_ssl() {
        // Zero host auto-disables SSL
        assert!(should_disable_quack_ssl("quack:0.0.0.0:9494", false));
        // Flag explicitly disables SSL
        assert!(should_disable_quack_ssl("quack:remote.example.com", true));
        // Non-zero host without flag or env keeps SSL
        assert!(!should_disable_quack_ssl("quack:remote.example.com", false));
    }

    #[test]
    fn test_cli_quack_disable_ssl_flags() {
        let cli = Cli::try_parse_from(&[
            "duckpack",
            "explore",
            "--db",
            "quack:0.0.0.0:9494",
            "--quack-disable-ssl",
        ]).unwrap();
        match cli.command {
            Commands::Explore { quack_disable_ssl, .. } => assert!(quack_disable_ssl),
            _ => panic!("Expected Explore command"),
        }

        let cli_insecure = Cli::try_parse_from(&[
            "duckpack",
            "explore",
            "--db",
            "quack:0.0.0.0:9494",
            "--insecure",
        ]).unwrap();
        match cli_insecure.command {
            Commands::Explore { quack_disable_ssl, .. } => assert!(quack_disable_ssl),
            _ => panic!("Expected Explore command"),
        }

        let cli_apply = Cli::try_parse_from(&[
            "duckpack",
            "apply",
            "--db",
            "quack:0.0.0.0:9494",
            "--quack-disable-ssl",
        ]).unwrap();
        match cli_apply.command {
            Commands::Apply { quack_disable_ssl, .. } => assert!(quack_disable_ssl),
            _ => panic!("Expected Apply command"),
        }
    }
}
