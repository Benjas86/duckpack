use std::panic;
use duckpack::diff;
use duckpack::engine::*;
use duckpack::ide;
use duckpack::tui;

use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use duckdb::Connection;
use std::path::{Path, PathBuf};

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
        /// Target database catalog name if multiple databases are attached
        #[arg(long, visible_alias = "catalog")]
        database: Option<String>,
        /// Allow destructive drops (tables/columns)
        #[arg(long)]
        force_drop: bool,
        /// Allow physical copy-and-swap table rewrites on DuckLake catalogs
        #[arg(long)]
        allow_lake_rewrite: bool,
        /// Apply automatically without showing the TUI
        #[arg(long)]
        auto_approve: bool,
        /// Launch the interactive TUI (default behavior unless --auto-approve is specified)
        #[arg(short, long)]
        interactive: bool,
        /// Dry run mode: output migration SQL script to this file instead of applying
        #[arg(long)]
        out: Option<String>,
        /// Quack authentication token
        #[arg(long)]
        quack_token: Option<String>,
        /// Disable SSL/TLS verification for Quack connection (plain HTTP)
        #[arg(long, visible_alias = "insecure", alias = "quack-insecure")]
        quack_disable_ssl: bool,
        /// Sync ONLY the declarative Quack RBAC policy (_duckpack_auth + live client_id_hash + global hooks) without running user schema migrations
        #[arg(long)]
        rbac_only: bool,
    },
    /// Show schema differences between project and target database
    Diff {
        /// The path to the DuckDB project directory
        #[arg(short, long, default_value = ".")]
        project_dir: PathBuf,

        /// The connection string or path to the target DuckDB database
        #[arg(short, long, default_value = "local.duckdb")]
        db: String,

        /// Target database catalog name if multiple databases are attached
        #[arg(long, visible_alias = "catalog")]
        database: Option<String>,

        /// Launch the interactive TUI to review schema diffs and migrations
        #[arg(short, long)]
        interactive: bool,

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
        /// Target database catalog name if multiple databases are attached
        #[arg(long, visible_alias = "catalog")]
        database: Option<String>,
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
    /// Manage DuckPack Python bindings in the active Python environment
    Python {
        #[command(subcommand)]
        command: PythonCommands,
    },
}

/// Subcommands for the `duckpack python` CLI helper
#[derive(Subcommand, Debug, PartialEq)]
enum PythonCommands {
    /// Install duckpack Python package into the active Python environment (auto-detects uv or pip)
    Install {
        /// Upgrade existing package
        #[arg(short = 'U', long)]
        upgrade: bool,
        /// Package installer to use: 'auto' (default), 'uv', or 'pip'
        #[arg(long, visible_alias = "method", default_value = "auto")]
        installer: String,
        /// Force installation using 'uv pip install'
        #[arg(long, conflicts_with = "pip")]
        uv: bool,
        /// Force installation using 'python -m pip install'
        #[arg(long, conflicts_with = "uv")]
        pip: bool,
    },
    /// Check whether duckpack Python package is installed in the active Python environment
    Status,
}

/// The main entry point for the DuckPack CLI.
/// Parses the arguments and delegates to the appropriate command handler.
fn main() -> Result<()> {
    panic::set_hook(Box::new(|info| { let bt = std::backtrace::Backtrace::force_capture(); let _ = std::fs::write("panic.log", format!("Panic: {:?}\n\n{}", info, bt)); }));
    let embedded_ver = duckpack::engine::embedded_duckdb_version();
    let dp_ver = duckpack::engine::duckpack_version();
    let version_str: &'static str = Box::leak(format!("v{} (DuckDB engine: {})", dp_ver, embedded_ver).into_boxed_str());
    let matches = Cli::command().version(version_str).get_matches();
    let mut cli = Cli::from_arg_matches(&matches)?;

    let default_dir = PathBuf::from(".");
    let project_dir: PathBuf = match &cli.command {
        Commands::Init { project_dir } => project_dir.clone(),
        Commands::Build { project_dir } => project_dir.clone(),
        Commands::Apply { project_dir, .. } => project_dir.clone(),
        Commands::Diff { project_dir, .. } => project_dir.clone(),
        Commands::Compile { project_dir, .. } => project_dir.clone(),
        Commands::Deploy { project_dir, .. } => project_dir.clone(),
        Commands::Explore { project_dir, .. } => project_dir.clone(),
        Commands::Python { .. } => default_dir,
    };
    let project_dir = &project_dir;

    // Apply target/env configuration overrides, defaults, and token resolution via unified TargetResolver
    match &mut cli.command {
        Commands::Apply { db, quack_token, quack_disable_ssl, .. } |
        Commands::Diff { db, quack_token, quack_disable_ssl, .. } |
        Commands::Explore { db, quack_token, quack_disable_ssl, .. } => {
            if quack_token.is_some() {
                eprintln!("Tip: Passing authentication tokens via CLI flags can expose credentials in shell history. Consider storing your token in .env (QUACK_TOKEN) or duckpack.toml.");
            }
            let spec = TargetResolver::resolve(
                project_dir,
                cli.env.as_deref(),
                Some(db.as_str()),
                quack_token.as_deref(),
                Some(*quack_disable_ssl),
                None,
            );
            let mut resolved_db = spec.db;
            if !spec.is_quack && !spec.is_motherduck && !std::path::Path::new(&resolved_db).is_absolute() && project_dir != std::path::Path::new(".") {
                resolved_db = project_dir.join(&resolved_db).to_string_lossy().to_string();
            }
            *db = resolved_db;
            *quack_token = spec.token;
            *quack_disable_ssl = spec.disable_ssl;
        }
        Commands::Deploy { db, .. } => {
            let spec = TargetResolver::resolve(
                project_dir,
                cli.env.as_deref(),
                Some(db.as_str()),
                None,
                None,
                None,
            );
            let mut resolved_db = spec.db;
            if !spec.is_quack && !spec.is_motherduck && !std::path::Path::new(&resolved_db).is_absolute() && project_dir != std::path::Path::new(".") {
                resolved_db = project_dir.join(&resolved_db).to_string_lossy().to_string();
            }
            *db = resolved_db;
        }
        _ => {}
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
        Commands::Diff { project_dir, db, database, interactive, quack_token, quack_disable_ssl } => {
            if !project_dir.exists() {
                eprintln!("Project directory {} does not exist. Run init first.", project_dir.display());
                return Ok(());
            }

            if *interactive {
                return run_diff_review_tui(
                    project_dir,
                    db,
                    database.as_deref(),
                    cli.env.as_deref(),
                    quack_token.as_deref(),
                    *quack_disable_ssl,
                    false,
                    None,
                );
            }

            let configured_catalog = database.clone().or_else(|| resolve_project_catalog(project_dir, cli.env.as_deref()));
            let mut target_conn = connect_target_db(db, quack_token.as_deref(), *quack_disable_ssl)?;
            let active_catalog = diff::validate_target_catalog_exec(&mut target_conn, configured_catalog.as_deref(), false)?;
            let diff_result = diff::diff_project(project_dir, &mut target_conn, Some(&active_catalog))?;

            let unignored_items: Vec<_> = diff_result.items.iter().filter(|i| !i.ignored).collect();
            if unignored_items.is_empty() {
                println!("Target database catalog '{}' is up to date (0 pending changes).", active_catalog);
            } else {
                println!("Found {} pending change(s) for catalog '{}':", unignored_items.len(), active_catalog);
                for item in unignored_items {
                    println!("  • {:?}: {}", item.item_type, item.obj_name);
                }
            }
        }
        Commands::Apply { project_dir, db, database, force_drop, allow_lake_rewrite, auto_approve, interactive: _, out, quack_token, quack_disable_ssl, rbac_only } => {
            if *allow_lake_rewrite {
                unsafe {
                    std::env::set_var("DUCKPACK_ALLOW_LAKE_REWRITE", "true");
                }
            }
            if !project_dir.exists() {
                eprintln!("Project directory {} does not exist. Run init first.", project_dir.display());
                return Ok(());
            }

            if *rbac_only {
                let mut target_conn = connect_target_db(db, quack_token.as_deref(), *quack_disable_ssl)?;
                let user_count = duckpack::quack_auth::deploy_quack_rbac_now(
                    &mut target_conn,
                    database.as_deref(),
                    project_dir,
                )?;
                println!(
                    "🦆 DuckPack RBAC: Synced {} declarative user(s) and activated Quack authentication/authorization hooks.",
                    user_count
                );
                return Ok(());
            }

            if *auto_approve {
                let configured_catalogs = if let Some(d) = database {
                    vec![d.clone()]
                } else {
                    let cats = resolve_project_catalogs(project_dir, cli.env.as_deref());
                    if cats.is_empty() {
                        let mut init_conn = connect_target_db(db, quack_token.as_deref(), *quack_disable_ssl)?;
                        let active = diff::validate_target_catalog_exec(&mut init_conn, None, true)?;
                        vec![active]
                    } else {
                        cats
                    }
                };

                let remote = is_remote(db);
                let mut total_applied = 0;
                for target_cat in &configured_catalogs {
                    let mut target_conn = connect_target_db(db, quack_token.as_deref(), *quack_disable_ssl)?;
                    let active_catalog = diff::validate_target_catalog_exec(&mut target_conn, Some(target_cat), true)?;
                    let diff_result = diff::diff_project(project_dir, &mut target_conn, Some(&active_catalog))?;

                    let pending_count = diff_result.items.iter().filter(|i| i.selected && !i.ignored).count();
                    if pending_count == 0 {
                        println!("Target catalog '{}' is up to date (0 pending changes).", active_catalog);
                        continue;
                    }

                    let apply_success = apply_diff_with_executor(&mut target_conn, &diff_result, *force_drop, remote, project_dir, Some(&active_catalog));
                    if apply_success.is_ok() {
                        println!("✓ Successfully applied {} change(s) automatically to catalog '{}'!", pending_count, active_catalog);
                        total_applied += pending_count;
                    } else {
                        eprintln!("Error applying schema to catalog '{}': {:?}", active_catalog, apply_success.err());
                        return Err(anyhow::anyhow!("Apply failed for catalog '{}'", active_catalog));
                    }
                }
                println!("🦆 DuckPack CI/CD: Migration completed. Applied {} total change(s) across {} catalog(s).", total_applied, configured_catalogs.len());
                return Ok(());
            }

            run_diff_review_tui(
                project_dir,
                db,
                database.as_deref(),
                cli.env.as_deref(),
                quack_token.as_deref(),
                *quack_disable_ssl,
                *force_drop,
                out.as_deref(),
            )?;
        }
        Commands::Compile { project_dir, out } => {
            println!("Compiling project {} into DuckPack: {}", project_dir.display(), out);
            let _shadow_conn = build_shadow_db(project_dir)?;
            
            // DuckPack format: A simple duckdb file.
            let _ = std::fs::remove_file(&out); // Remove if exists
            let mut out_conn = Connection::open(&out).with_context(|| "Failed to create .duckpack file")?;
            println!("Packaging schemas, macros, tables, views, and triggers...");
            compile_project_declarations(&mut out_conn, project_dir)?;
            println!("Compilation complete! Artifact: {}", out);
        }
        Commands::Explore { project_dir, db, database, remote, port, auto_install, quack_token, quack_disable_ssl } => {
            if let Some(r) = remote {
                let mut remote_bin = "duckpack".to_string();
                let mut remote_tmp_bin = None;
                let rand_suffix = &generate_random_hex_token()[..16];
                
                if *auto_install {
                    println!("Auto-installing DuckPack companion binary for exploration...");
                    let current_exe = std::env::current_exe().with_context(|| "Failed to get current executable path")?;
                    let tmp_bin = format!("/tmp/duckpack_bin_{}", rand_suffix);
                    
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
                        .arg(format!("chmod +x {}", shell_quote(&tmp_bin)))
                        .status()?;
                        
                    if !chmod_status.success() {
                        anyhow::bail!("Failed to make binary executable on remote server.");
                    }
                    
                    remote_bin = tmp_bin.clone();
                    remote_tmp_bin = Some(tmp_bin);
                }

                let remote_tmp_proj = format!("/tmp/duckpack_explore_{}", rand_suffix);
                let _ = std::process::Command::new("ssh")
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(r)
                    .arg(format!("mkdir -p {}/queries", shell_quote(&remote_tmp_proj)))
                    .status();
                
                println!("Launching remote TUI over SSH...");
                let mut remote_cmd = String::new();
                if let Some(token) = quack_token {
                    remote_cmd.push_str(&format!("QUACK_TOKEN={} ", shell_quote(token)));
                }
                remote_cmd.push_str(&format!(
                    "{} explore --project-dir {} --db {}",
                    shell_quote(&remote_bin),
                    shell_quote(&remote_tmp_proj),
                    shell_quote(db)
                ));
                if let Some(cat) = database {
                    remote_cmd.push_str(&format!(" --database {}", shell_quote(cat)));
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
                let mut cleanup_cmd = format!("rm -rf {}", shell_quote(&remote_tmp_proj));
                if let Some(tmp_bin) = remote_tmp_bin {
                    cleanup_cmd.push_str(&format!(" {}", shell_quote(&tmp_bin)));
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
            if project_dir == std::path::PathBuf::from(".") {
                let discovered = discover_project_dir(&project_dir);
                if discovered != project_dir {
                    project_dir = discovered;
                } else if !db.starts_with("quack:") && !is_remote(&db) {
                    let db_path = std::path::PathBuf::from(&db);
                    if let Some(parent) = db_path.parent() {
                        if parent != std::path::Path::new("") {
                            project_dir = parent.to_path_buf();
                        }
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
            let configured_catalog = database.clone().or_else(|| resolve_project_catalog(&project_dir, cli.env.as_deref()));
            if let Some(cat) = configured_catalog {
                let _ = target_conn.execute_batch(&format!("USE \"{}\";", cat.replace('\"', "\"\"")));
            }

            if db.starts_with("quack:") {
                cleanup_orphaned_quack_bridges(&target_conn);
            }

            let ide_res = ide::run_ide_loop(&mut terminal, &target_conn, &project_dir, &db, *quack_disable_ssl);

            ratatui::crossterm::terminal::disable_raw_mode()?;
            ratatui::crossterm::execute!(
                terminal.backend_mut(),
                ratatui::crossterm::terminal::LeaveAlternateScreen,
                ratatui::crossterm::event::DisableMouseCapture
            )?;

            ide_res?;
        }
        Commands::Deploy { project_dir, remote, db, port, auto_install } => {
            println!("Deploying {} to {} at {}", project_dir.display(), remote, db);
            
            let rand_suffix = &generate_random_hex_token()[..16];
            let tmp_pack = std::env::temp_dir()
                .join(format!("duckpack_deploy_{}.duckpack", rand_suffix))
                .to_string_lossy()
                .to_string();
            
            println!("1. Compiling local DuckPack artifact...");
            let _ = std::fs::remove_file(&tmp_pack);
            let mut out_conn = Connection::open(&tmp_pack).with_context(|| "Failed to create temp .duckpack file")?;
            compile_project_declarations(&mut out_conn, project_dir)?;
            out_conn.close().map_err(|e| anyhow::anyhow!("Failed to close DuckPack: {:?}", e.1))?;
            
            let remote_tmp_pack = format!("/tmp/deploy_{}.duckpack", rand_suffix);
            
            println!("2. Transferring artifact to remote server (scp)...");
            let scp_status = std::process::Command::new("scp")
                .arg("-P")
                .arg(port.to_string())
                .arg(&tmp_pack)
                .arg(format!("{}:{}", remote, remote_tmp_pack))
                .status()?;
            
            if !scp_status.success() {
                let _ = std::fs::remove_file(&tmp_pack);
                anyhow::bail!("Failed to SCP artifact to remote server.");
            }
            
            let mut remote_bin = "duckpack".to_string();
            let mut remote_tmp_bin = None;
            
            if *auto_install {
                println!("2.5. Auto-installing DuckPack companion binary...");
                let current_exe = std::env::current_exe().with_context(|| "Failed to get current executable path")?;
                let tmp_bin = format!("/tmp/duckpack_bin_{}", rand_suffix);
                
                let scp_bin_status = std::process::Command::new("scp")
                    .arg("-P")
                    .arg(port.to_string())
                    .arg(&current_exe)
                    .arg(format!("{}:{}", remote, tmp_bin))
                    .status()?;
                    
                if !scp_bin_status.success() {
                    let _ = std::fs::remove_file(&tmp_pack);
                    anyhow::bail!("Failed to SCP binary to remote server.");
                }
                
                // Make the binary executable
                let chmod_status = std::process::Command::new("ssh")
                    .arg("-p")
                    .arg(port.to_string())
                    .arg(remote)
                    .arg(format!("chmod +x {}", shell_quote(&tmp_bin)))
                    .status()?;
                    
                if !chmod_status.success() {
                    let _ = std::fs::remove_file(&tmp_pack);
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
                .arg(format!(
                    "{} apply --project-dir {} --db {} --auto-approve",
                    shell_quote(&remote_bin),
                    shell_quote(&remote_tmp_pack),
                    shell_quote(db)
                ))
                .status()?;
                
            if !ssh_status.success() {
                println!("Deployment failed during remote execution.");
            } else {
                println!("Deployment successful!");
            }
            
            println!("4. Cleaning up temporary artifacts...");
            let _ = std::fs::remove_file(&tmp_pack);
            
            let mut cleanup_cmd = format!("rm -f {}", shell_quote(&remote_tmp_pack));
            if let Some(tmp_bin) = remote_tmp_bin {
                cleanup_cmd.push_str(&format!(" {}", shell_quote(&tmp_bin)));
            }
            
            let _ = std::process::Command::new("ssh")
                .arg("-p")
                .arg(port.to_string())
                .arg(remote)
                .arg(cleanup_cmd)
                .status();
        }
        Commands::Python { command } => {
            match command {
                PythonCommands::Status => {
                    let embedded_ver = duckpack::engine::embedded_duckdb_version();
                    let dp_ver = duckpack::engine::duckpack_version();
                    let py_bin = resolve_active_python_binary();
                    let detected_installer =
                        resolve_python_installer("auto", false, false, &py_bin);
                    println!("🦆 DuckPack CLI: v{}", dp_ver);
                    println!("   Embedded DuckDB engine: {}", embedded_ver);
                    println!(
                        "\nChecking active Python environment ({}, installer: {})...",
                        py_bin, detected_installer
                    );

                    let duckpack_py_out = std::process::Command::new(&py_bin)
                        .args(["-c", "import duckpack; print(duckpack.__version__)"])
                        .output();
                    match duckpack_py_out {
                        Ok(out) if out.status.success() => {
                            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            let dp_str = if ver.starts_with('v') { ver } else { format!("v{}", ver) };
                            println!("✓ DuckPack Python package: {} (installed)", dp_str);
                        }
                        _ => {
                            println!("✗ DuckPack Python package: not installed");
                            println!(
                                "  Run 'duckpack python install' (or 'duckpack python install --uv') to install it."
                            );
                        }
                    }

                    let duckdb_py_out = std::process::Command::new(&py_bin)
                        .args(["-c", "import duckdb; print(duckdb.__version__)"])
                        .output();
                    match duckdb_py_out {
                        Ok(out) if out.status.success() => {
                            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            let ddb_str = if ver.starts_with('v') { ver } else { format!("v{}", ver) };
                            println!("✓ Python DuckDB package: {}", ddb_str);
                        }
                        _ => {
                            println!("- Python DuckDB package: not found");
                        }
                    }
                }
                PythonCommands::Install {
                    upgrade,
                    installer,
                    uv,
                    pip,
                } => {
                    let py_bin = resolve_active_python_binary();
                    let chosen = resolve_python_installer(installer, *uv, *pip, &py_bin);
                    let (prog, args) = build_python_install_command(chosen, &py_bin, *upgrade);
                    println!(
                        "Installing DuckPack Python package via '{}' ({} {})...",
                        chosen,
                        prog,
                        args.join(" ")
                    );
                    let status = std::process::Command::new(&prog).args(&args).status();
                    match status {
                        Ok(s) if s.success() => {
                            println!(
                                "✓ Successfully installed DuckPack Python package via '{}'!",
                                chosen
                            );
                        }
                        Ok(s) => {
                            if chosen == "pip" && !*pip && installer == "auto" && is_uv_available()
                            {
                                println!(
                                    "pip exited with {}; falling back to 'uv pip install'...",
                                    s
                                );
                                let (uv_prog, uv_args) =
                                    build_python_install_command("uv", &py_bin, *upgrade);
                                if let Ok(uv_s) =
                                    std::process::Command::new(&uv_prog).args(&uv_args).status()
                                {
                                    if uv_s.success() {
                                        println!(
                                            "✓ Successfully installed DuckPack Python package via 'uv'!"
                                        );
                                        return Ok(());
                                    }
                                }
                            }
                            eprintln!("{} install exited with status {}", chosen, s);
                        }
                        Err(e) => {
                            eprintln!("Failed to execute '{}': {}", prog, e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Quotes a string as a single-quoted POSIX shell token (`'...'`, escaping embedded `'` as `'\''`).
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// Resolves the active Python interpreter binary (`$VIRTUAL_ENV/bin/python`, `./.venv/bin/python`, or `python3`).
fn resolve_active_python_binary() -> String {
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let unix_py = PathBuf::from(&venv).join("bin").join("python");
        if unix_py.exists() {
            return unix_py.to_string_lossy().to_string();
        }
        let win_py = PathBuf::from(&venv).join("Scripts").join("python.exe");
        if win_py.exists() {
            return win_py.to_string_lossy().to_string();
        }
    }
    let local_unix = PathBuf::from(".venv").join("bin").join("python");
    if local_unix.exists() {
        return local_unix.to_string_lossy().to_string();
    }
    let local_win = PathBuf::from(".venv").join("Scripts").join("python.exe");
    if local_win.exists() {
        return local_win.to_string_lossy().to_string();
    }
    "python3".to_string()
}

fn is_uv_available() -> bool {
    std::process::Command::new("uv")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_uv_managed_environment() -> bool {
    if std::env::var("UV_PROJECT_ENVIRONMENT").is_ok() || Path::new("uv.lock").exists() {
        return true;
    }
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        if let Ok(cfg) = std::fs::read_to_string(PathBuf::from(venv).join("pyvenv.cfg")) {
            if cfg.lines().any(|l| l.trim_start().starts_with("uv =")) {
                return true;
            }
        }
    }
    if let Ok(cfg) = std::fs::read_to_string(Path::new(".venv").join("pyvenv.cfg")) {
        if cfg.lines().any(|l| l.trim_start().starts_with("uv =")) {
            return true;
        }
    }
    false
}

/// Determines which Python package installer (`"uv"` or `"pip"`) to use based on CLI flags and environment introspection.
fn resolve_python_installer(
    explicit_installer: &str,
    force_uv: bool,
    force_pip: bool,
    py_bin: &str,
) -> &'static str {
    if force_uv || explicit_installer.eq_ignore_ascii_case("uv") {
        return "uv";
    }
    if force_pip || explicit_installer.eq_ignore_ascii_case("pip") {
        return "pip";
    }
    if is_uv_available() {
        if is_uv_managed_environment() {
            return "uv";
        }
        let pip_ok = std::process::Command::new(py_bin)
            .args(["-m", "pip", "--version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !pip_ok {
            return "uv";
        }
    }
    "pip"
}

/// Builds the `(program, args)` tuple for installing `duckpack` via `"uv"` or `"pip"`.
fn build_python_install_command(
    installer: &str,
    py_bin: &str,
    upgrade: bool,
) -> (String, Vec<String>) {
    if installer.eq_ignore_ascii_case("uv") {
        let mut args = vec!["pip".to_string(), "install".to_string()];
        if py_bin != "python3" {
            args.push("--python".to_string());
            args.push(py_bin.to_string());
        } else if std::env::var("VIRTUAL_ENV").is_err() && !Path::new(".venv").exists() {
            args.push("--system".to_string());
        }
        if upgrade {
            args.push("--upgrade".to_string());
        }
        args.push("duckpack".to_string());
        ("uv".to_string(), args)
    } else {
        let mut args = vec!["-m".to_string(), "pip".to_string(), "install".to_string()];
        if upgrade {
            args.push("--upgrade".to_string());
        }
        args.push("duckpack".to_string());
        (py_bin.to_string(), args)
    }
}

/// Launches the interactive visual diff & declarative migration review TUI.
///
/// Can be invoked via `duckpack diff --interactive` (or `-i`) or `duckpack apply` (default mode).
fn run_diff_review_tui(
    project_dir: &PathBuf,
    db: &str,
    database: Option<&str>,
    env: Option<&str>,
    quack_token: Option<&str>,
    quack_disable_ssl: bool,
    force_drop: bool,
    out: Option<&str>,
) -> Result<()> {
    let configured_catalog = database
        .map(String::from)
        .or_else(|| resolve_project_catalog(project_dir, env));
    let conn = connect_target_db(db, quack_token, quack_disable_ssl)?;
    if db.starts_with("quack:") {
        cleanup_orphaned_quack_bridges(&conn);
    }

    if let Some(out_file) = out {
        let mut exec = &conn;
        let session = tui::DiffReviewSession::new(
            &mut exec,
            project_dir,
            db,
            configured_catalog.as_deref(),
            force_drop,
        )?;
        let script = session.render_dry_run_script();
        if let Err(e) = std::fs::write(out_file, script) {
            eprintln!("Error writing dry-run script: {}", e);
        } else {
            println!("Dry-run script successfully written to {}", out_file);
        }
        return Ok(());
    }

    ratatui::crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    ratatui::crossterm::execute!(
        stdout,
        ratatui::crossterm::terminal::EnterAlternateScreen,
        ratatui::crossterm::event::EnableMouseCapture
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let session_res = tui::run_diff_review_session(
        &mut terminal,
        &conn,
        project_dir,
        db,
        configured_catalog.as_deref(),
        force_drop,
        quack_disable_ssl,
    );

    let _ = ratatui::crossterm::terminal::disable_raw_mode();
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::terminal::LeaveAlternateScreen,
        ratatui::crossterm::event::DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    let summary = session_res?;
    if summary.total_applied_count > 0 {
        println!(
            "🦆 DuckPack: Applied {} total change(s) across catalog(s): [{}].",
            summary.total_applied_count,
            summary.applied_catalogs.join(", ")
        );
    } else {
        println!("🦆 DuckPack: Exited without applying changes.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_remote() {
        assert!(is_remote("md:my_db"));
        assert!(is_remote("motherduck:"));
        assert!(!is_remote("local.db"));
        assert!(!is_remote("/var/lib/duckdb/prod.db"));
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
        let cli = Cli::try_parse_from([
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

        let cli_insecure = Cli::try_parse_from([
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

        let cli_apply = Cli::try_parse_from([
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

    #[test]
    fn test_cli_python_subcommands() {
        let cli_status = Cli::try_parse_from(["duckpack", "python", "status"]).unwrap();
        match cli_status.command {
            Commands::Python { command } => assert_eq!(command, PythonCommands::Status),
            _ => panic!("Expected Python status command"),
        }

        let cli_install = Cli::try_parse_from(["duckpack", "python", "install"]).unwrap();
        match cli_install.command {
            Commands::Python { command } => assert_eq!(
                command,
                PythonCommands::Install {
                    upgrade: false,
                    installer: "auto".to_string(),
                    uv: false,
                    pip: false,
                }
            ),
            _ => panic!("Expected Python install command"),
        }

        let cli_upgrade = Cli::try_parse_from(["duckpack", "python", "install", "--upgrade"]).unwrap();
        match cli_upgrade.command {
            Commands::Python { command } => assert_eq!(
                command,
                PythonCommands::Install {
                    upgrade: true,
                    installer: "auto".to_string(),
                    uv: false,
                    pip: false,
                }
            ),
            _ => panic!("Expected Python install upgrade command"),
        }

        let cli_uv = Cli::try_parse_from(["duckpack", "python", "install", "--uv", "-U"]).unwrap();
        match cli_uv.command {
            Commands::Python { command } => assert_eq!(
                command,
                PythonCommands::Install {
                    upgrade: true,
                    installer: "auto".to_string(),
                    uv: true,
                    pip: false,
                }
            ),
            _ => panic!("Expected Python install --uv command"),
        }
        assert_eq!(resolve_python_installer("auto", true, false, "python3"), "uv");
        assert_eq!(resolve_python_installer("uv", false, false, "python3"), "uv");
        assert_eq!(resolve_python_installer("pip", false, false, "python3"), "pip");
        let (uv_prog, uv_args) = build_python_install_command("uv", ".venv/bin/python", true);
        assert_eq!(uv_prog, "uv");
        assert_eq!(
            uv_args,
            vec!["pip", "install", "--python", ".venv/bin/python", "--upgrade", "duckpack"]
        );
    }

    #[test]
    fn test_cli_interactive_flags() {
        let cli_diff = Cli::try_parse_from(["duckpack", "diff", "--interactive"]).unwrap();
        match cli_diff.command {
            Commands::Diff { interactive, .. } => assert!(interactive),
            _ => panic!("Expected Diff command"),
        }

        let cli_diff_short = Cli::try_parse_from(["duckpack", "diff", "-i"]).unwrap();
        match cli_diff_short.command {
            Commands::Diff { interactive, .. } => assert!(interactive),
            _ => panic!("Expected Diff command"),
        }

        let cli_apply = Cli::try_parse_from(["duckpack", "apply", "--interactive"]).unwrap();
        match cli_apply.command {
            Commands::Apply { interactive, .. } => assert!(interactive),
            _ => panic!("Expected Apply command"),
        }

        let cli_apply_short = Cli::try_parse_from(["duckpack", "apply", "-i"]).unwrap();
        match cli_apply_short.command {
            Commands::Apply { interactive, .. } => assert!(interactive),
            _ => panic!("Expected Apply command"),
        }
    }
}

