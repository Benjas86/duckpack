mod diff;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use duckdb::Connection;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

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

        /// The path to the target DuckDB database file
        #[arg(short, long, default_value = "local.duckdb")]
        db: PathBuf,
        
        /// Allow destructive drops (tables/columns)
        #[arg(long)]
        force_drop: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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
        Commands::Apply { project_dir, db, force_drop } => {
            println!("Compiling Shadow DB from {}...", project_dir.display());
            let shadow_conn = build_shadow_db(project_dir)?;
            
            println!("Connecting to Target DB at {}...", db.display());
            // Create target db if it doesn't exist
            let target_conn = Connection::open(db).with_context(|| "Failed to connect to Target DB")?;
            
            // 1. Get Schemas
            let shadow_schema = diff::get_schema(&shadow_conn)?;
            let target_schema = diff::get_schema(&target_conn)?;
            
            let shadow_views = diff::get_views(&shadow_conn)?;
            let target_views = diff::get_views(&target_conn)?;

            let shadow_macros = diff::get_macros(&shadow_conn)?;
            let target_macros = diff::get_macros(&target_conn)?;

            // 2. Compute Diff
            let mut diff_result = diff::compute_diff(
                &shadow_schema, 
                &target_schema, 
                &shadow_views, 
                &target_views, 
                &shadow_macros, 
                &target_macros, 
                project_dir
            );
            
            // 3. Show TUI
            let should_apply = tui::run_tui(&mut diff_result, *force_drop)?;
            
            // 4. Apply Changes
            if should_apply {
                println!("Applying changes to {}...", db.display());
                
                // Create backup first
                let mut b_path = db.clone();
                b_path.set_extension("duckdb.bak");
                if db.exists() {
                    fs::copy(db, &b_path).with_context(|| "Failed to create database backup")?;
                }
                
                let apply_success = apply_diff(&target_conn, &diff_result, *force_drop);
                
                if apply_success.is_ok() {
                    println!("Changes applied successfully!");
                    if b_path.exists() {
                        fs::remove_file(&b_path).ok();
                    }
                } else {
                    eprintln!("Error applying schema: {:?}", apply_success.err());
                    println!("Rolling back database from backup...");
                    drop(target_conn); 
                    if b_path.exists() {
                        fs::copy(&b_path, db).with_context(|| "Failed to restore backup")?;
                        fs::remove_file(&b_path).ok();
                    }
                }
            } else {
                println!("Operation cancelled by user.");
            }
        }
    }

    Ok(())
}

fn apply_diff(conn: &Connection, diff: &diff::DiffResult, force_drop: bool) -> Result<()> {
    let mut views_to_apply = Vec::new();

    for item in &diff.items {
        if !item.selected {
            continue;
        }

        match item.item_type {
            diff::DiffItemType::CreateTable | diff::DiffItemType::AlterTable | diff::DiffItemType::CreateMacro => {
                conn.execute_batch(&item.sql)?;
            }
            diff::DiffItemType::DropTable | diff::DiffItemType::DropColumn | diff::DiffItemType::DropView | diff::DiffItemType::DropMacro => {
                if force_drop {
                    conn.execute_batch(&item.sql)?;
                }
            }
            diff::DiffItemType::CreateView => {
                views_to_apply.push(item.sql.clone());
            }
        }
    }

    if !views_to_apply.is_empty() {
        execute_views_sqls_with_retry(conn, views_to_apply)?;
    }

    Ok(())
}

fn init_project(project_dir: &PathBuf) -> Result<()> {
    let dirs = ["tables", "views", "macros", "seeds"];
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

- `tables/`: Contains `CREATE TABLE` statements (one per file).
- `views/`: Contains `CREATE OR REPLACE VIEW` statements.
- `macros/`: Contains `CREATE OR REPLACE MACRO` statements.
- `seeds/`: Contains static CSVs or seed scripts.
- `duck-project.toml`: Project configuration and targets.

## CLI Commands

### 1. Build & Validate
Validates your project syntax and verifies dependencies by compiling it against an in-memory Shadow DB.

```bash
duck-migrate build --project-dir .
```

### 2. Apply 
Computes the diff against the target database and opens an interactive TUI to review and selectively apply the changes.

```bash
duck-migrate apply --project-dir . --db local.duckdb
```

**TUI Controls:**
- `Up/Down`: Navigate the list of proposed changes.
- `Space`: Toggle selection (checked items will be applied, unchecked items will be ignored).
- `Enter`: Apply the selected changes transactionally.
- `Esc / q`: Cancel the operation.

**Options:**
- `--force-drop`: By default, the engine operates in 'Safe Mode' and ignores deleted tables/columns to prevent data loss. Pass this flag to allow destructive `DROP TABLE` and `DROP COLUMN` commands.
"#;
        fs::write(&readme_path, readme_content).with_context(|| "Failed to write README.md")?;
        println!("Created documentation file: {}", readme_path.display());
    }

    Ok(())
}

fn build_shadow_db(project_dir: &PathBuf) -> Result<Connection> {
    let conn = Connection::open_in_memory().with_context(|| "Failed to create Shadow DB")?;
    
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
        execute_views_with_retry(&conn, &views_dir)?;
    }

    Ok(conn)
}

fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            let sql = fs::read_to_string(&path)?;
            conn.execute_batch(&sql).with_context(|| format!("Failed to execute {}", path.display()))?;
        }
    }
    Ok(())
}

fn execute_views_sqls_with_retry(conn: &Connection, mut pending_views: Vec<String>) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_views.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_views {
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
        pending_views = remaining;
    }

    if !pending_views.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context("Failed to compile views due to unresolvable dependencies or syntax errors");
        }
    }

    Ok(())
}

fn execute_views_with_retry(conn: &Connection, dir: &Path) -> Result<()> {
    let mut pending_views = Vec::new();
    
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            pending_views.push(fs::read_to_string(&path)?);
        }
    }

    execute_views_sqls_with_retry(conn, pending_views)
}
