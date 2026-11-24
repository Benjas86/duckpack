use anyhow::{Context, Result};
use duckdb::{AccessMode, Config, Connection};
use std::fs;
use std::path::{Path, PathBuf};

use crate::diff;
use crate::engine::{open_duckdb_connection, DbExecutor};
use crate::env_store::write_restricted_env_file;

/// Scaffolds a new declarative database project by generating the standard directory structure,
/// configuration files (`duckpack.toml`), environment templates, and documentation.
pub fn init_project(project_dir: &Path) -> Result<()> {
    let dirs = [
        "schemas",
        "types",
        "sequences",
        "tables",
        "views",
        "macros",
        "triggers",
        "queries",
        "seeds",
        "scripts/pre-deploy",
        "scripts/post-deploy",
    ];
    for dir in dirs.iter() {
        let path = project_dir.join(dir);
        if !path.exists() {
            fs::create_dir_all(&path)
                .with_context(|| format!("Failed to create directory {}", path.display()))?;
            println!("Created directory: {}", path.display());
        }
    }

    let config_path = project_dir.join("duckpack.toml");
    if !config_path.exists() {
        let default_config = r#"[project]
name = "my_duckdb_project"
version = "0.1.0"
description = "A declarative DuckDB database project"

[target]
default_db = "local.duckdb"

# Remote Quack example:
# host = "analytics.company.internal"
# port = 9494
# database = "analytics"
# token_env = "QUACK_TOKEN"
# insecure = false

# Multi-environment targets:
# [env.staging]
# db = "quack:staging.company.internal:9494"
# database = "analytics_stg"
# token_env = "STAGING_QUACK_TOKEN"

# [env.prod]
# db = "quack:prod.company.internal:9494"
# database = "analytics"
# token_env = "PROD_QUACK_TOKEN"
"#;
        fs::write(&config_path, default_config).with_context(|| "Failed to write duckpack.toml")?;
        println!("Created config file: {}", config_path.display());
    }

    let readme_path = project_dir.join("README.md");
    if !readme_path.exists() {
        let readme_content = r#"# DuckDB Database Project

Welcome to your declarative DuckDB project! This directory contains the desired state of your database schema.

## Project Structure

- `schemas/`: Contains `CREATE SCHEMA` statements. Schemas deleted from this folder will be dropped **CASCADE** on the target DB!
- `types/`: Contains `CREATE TYPE` statements (enums, custom data types).
- `sequences/`: Contains `CREATE SEQUENCE` statements (compiled before tables to support `DEFAULT nextval('...')`).
- `tables/`: Contains `CREATE TABLE` statements (one per file). Subdirectories supported for nested schemas.
- `views/`: Contains `CREATE VIEW` statements (engine auto-forces `OR REPLACE`).
- `macros/`: Contains `CREATE MACRO` statements.
- `triggers/`: Contains `CREATE TRIGGER` statements (table-scoped triggers).
- `queries/`: Save your ad-hoc `.sql` queries and scratchpad files here.
- `seeds/`: Contains static CSVs or seed scripts.
- `scripts/`: Contains `pre-deploy` and `post-deploy` scripts.
- `duckpack.toml`: Project configuration and targets.
- `.env`: Environment variables for template interpolation. Write `${VAR_NAME}` in your SQL, and the engine will dynamically replace it!
- `.duckpackignore`: Specifies tables, views, sequences, triggers, or file paths from any directory (including `scripts/pre-deploy` and `scripts/post-deploy`) to ignore during compilation and deployment (wildcards supported).
"#;
        fs::write(&readme_path, readme_content).with_context(|| "Failed to write README.md")?;
        println!("Created documentation file: {}", readme_path.display());
    }

    let env_path = project_dir.join(".env");
    if !env_path.exists() {
        let env_content = r#"# Environment variables for DuckPack SQL template interpolation and Quack credentials.
# Example SQL template variable: 
# ENV=dev

# Remote Quack server authentication token (avoids exposing credentials in shell history):
# QUACK_TOKEN=your_quack_token_here
"#;
        write_restricted_env_file(&env_path, env_content).with_context(|| "Failed to write .env")?;
        println!("Created environment file: {}", env_path.display());
    }

    let ignore_path = project_dir.join(".duckpackignore");
    if !ignore_path.exists() {
        let ignore_content = r#"# Specify tables, views, sequences, triggers, or file paths from any directory to ignore.
# Wildcards (* and ?) and relative directory paths are supported.
# Examples:
# temp_*
# *_backup
# scratch_*
# scripts/pre-deploy/01_one_time_seed.sql
# scripts/post-deploy/*
# archive/
"#;
        fs::write(&ignore_path, ignore_content)
            .with_context(|| "Failed to write .duckpackignore")?;
        println!("Created ignore file: {}", ignore_path.display());
    }

    let gitignore_path = project_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = ".env*\n*.duckdb\n*.history.sql\nREADME.md\n*.csv\n*.duckpack\nduckpack.log\nmigration.sql\n";
        fs::write(&gitignore_path, gitignore_content)
            .with_context(|| "Failed to write .gitignore")?;
        println!("Created .gitignore file: {}", gitignore_path.display());
    }

    Ok(())
}

/// Compiles all declarative SQL directories (`schemas`, `types`, `sequences`, `macros`, `tables`, `views`, `triggers`)
/// against any `DbExecutor` in dependency-safe topological order with retry, filtering out files matched by `.duckpackignore`.
pub fn compile_project_declarations(exec: &mut dyn DbExecutor, project_dir: &Path) -> Result<()> {
    let ignore_rules = load_ignore_rules(project_dir);
    for dir_name in [
        "schemas",
        "types",
        "sequences",
        "macros",
        "tables",
        "views",
        "triggers",
    ] {
        let dir = project_dir.join(dir_name);
        if dir.exists() {
            execute_directory_filtered(&dir, project_dir, &ignore_rules, |sql| {
                exec.execute_batch(sql)
            })?;
        }
    }
    Ok(())
}

/// Compiles all local `.sql` files into an isolated, sandboxed in-memory DuckDB connection
/// (`enable_external_access = false`, `allow_community_extensions = false`, `lock_configuration = true`).
/// When `project_dir` is a `.duckpack` artifact, opens it strictly in `ReadOnly` mode (H4).
pub fn build_shadow_db(project_dir: &Path) -> Result<Connection> {
    if project_dir.is_file() && project_dir.extension().unwrap_or_default() == "duckpack" {
        let cfg = Config::default().access_mode(AccessMode::ReadOnly)?;
        return Connection::open_with_flags(project_dir, cfg)
            .with_context(|| "Failed to open .duckpack artifact read-only");
    }

    let mut conn = open_duckdb_connection().with_context(|| "Failed to create Shadow DB")?;
    // H4: Sandbox Shadow DB so untrusted project SQL cannot touch host files, install extensions,
    // or issue outbound HTTP requests during build/diff/compile validation.
    conn.execute_batch(
        "SET enable_external_access = false;\n\
         SET allow_community_extensions = false;\n\
         SET lock_configuration = true;",
    )
    .with_context(|| "Failed to sandbox Shadow DB")?;
    compile_project_declarations(&mut conn, project_dir)?;

    Ok(conn)
}

/// Recursively collects `.sql` files, filtering out any files or subdirectories matched by `ignore_rules`.
pub fn collect_sql_files_filtered(
    dir: &Path,
    project_dir: &Path,
    ignore_rules: &[String],
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if is_file_ignored(dir, project_dir, ignore_rules) {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if !is_file_ignored(&path, project_dir, ignore_rules) {
                collect_sql_files_filtered(&path, project_dir, ignore_rules, out)?;
            }
        } else if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            if !is_file_ignored(&path, project_dir, ignore_rules) {
                out.push(path);
            }
        }
    }
    Ok(())
}

/// Helper to collect all SQL files in `dir` respecting project `.duckpackignore`.
pub fn collect_sql_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let project_dir = discover_project_dir(dir);
    let ignore_rules = load_ignore_rules(&project_dir);
    collect_sql_files_filtered(dir, &project_dir, &ignore_rules, out)
}

/// Iterates over a directory of `.sql` files and executes them using a custom executor function.
/// Respects ignore rules from `.duckpackignore`, `.duckdbignore`, or `.duckignore`.
pub fn execute_directory_custom<F>(dir: &Path, exec_fn: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let project_dir = discover_project_dir(dir);
    let ignore_rules = load_ignore_rules(&project_dir);
    execute_directory_filtered(dir, &project_dir, &ignore_rules, exec_fn)
}

/// Iterates over a directory of `.sql` files, filtering out files matched by `ignore_rules`.
pub fn execute_directory_filtered<F>(
    dir: &Path,
    project_dir: &Path,
    ignore_rules: &[String],
    exec_fn: F,
) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut files = Vec::new();
    collect_sql_files_filtered(dir, project_dir, ignore_rules, &mut files)?;
    let mut pending_sqls = Vec::new();
    for path in files {
        pending_sqls.push(diff::read_sql_with_env(&path)?);
    }

    execute_sqls_with_retry_custom(pending_sqls, exec_fn)
        .context(format!("Failed to compile SQL files in {}", dir.display()))
}

/// Iterates over a directory of `.sql` files and executes them against a connection.
pub fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    execute_directory_custom(dir, |sql| {
        conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e))
    })
}

/// Executes arbitrary SQL batches with dependency-order retries.
pub fn execute_sqls_with_retry_custom<F>(
    mut pending_sqls: Vec<String>,
    mut exec_fn: F,
) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match exec_fn(&sql) {
                Ok(_) => {
                    progress_made = true;
                }
                Err(e) => {
                    last_error = Some(e);
                    remaining.push(sql);
                }
            }
        }
        pending_sqls = remaining;
    }

    if !pending_sqls.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context(
                "Failed to compile SQLs due to unresolvable dependencies or syntax errors",
            );
        }
    }

    Ok(())
}

/// Executes arbitrary SQL batches against a connection with dependency-order retries.
pub fn execute_sqls_with_retry(conn: &Connection, pending_sqls: Vec<String>) -> Result<()> {
    execute_sqls_with_retry_custom(pending_sqls, |sql| {
        conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e))
    })
}

/// Loads ignore patterns from `.duckpackignore`, `.duckdbignore`, or legacy `.duckignore`.
pub fn load_ignore_rules(project_dir: &Path) -> Vec<String> {
    let mut rules = Vec::new();
    let candidates = [".duckpackignore", ".duckdbignore", ".duckignore"];
    for name in candidates {
        let path = project_dir.join(name);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        if !rules.contains(&trimmed.to_string()) {
                            rules.push(trimmed.to_string());
                        }
                    }
                }
            }
        }
    }
    rules
}

/// Matches a string against a simple glob pattern where `*` matches 0 or more characters
/// and `?` matches any single character.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == text || pattern == "*" {
        return true;
    }
    let mut regex_str = String::with_capacity(pattern.len() * 2 + 2);
    regex_str.push('^');
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
                regex_str.push_str(".*");
            }
            '?' => regex_str.push('.'),
            '.' | '(' | ')' | '+' | '|' | '^' | '$' | '@' | '%' | '{' | '}' | '[' | ']' | '\\' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            other => regex_str.push(other),
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

/// Checks whether a file or directory path matches any ignore rule from `.duckpackignore`.
pub fn is_file_ignored(file_path: &Path, project_dir: &Path, rules: &[String]) -> bool {
    if rules.is_empty() {
        return false;
    }

    let norm_path_str = file_path.to_string_lossy().replace('\\', "/");
    let norm_proj_str = project_dir.to_string_lossy().replace('\\', "/");

    let rel_str = if norm_path_str.starts_with(&norm_proj_str) {
        let stripped = &norm_path_str[norm_proj_str.len()..];
        stripped.trim_start_matches('/').to_string()
    } else {
        match file_path.strip_prefix(project_dir) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => norm_path_str.clone(),
        }
    };

    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_stem = file_path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    for raw_rule in rules {
        let pattern = raw_rule.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        let pattern = pattern.replace('\\', "/");
        let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);

        if pattern.is_empty() {
            continue;
        }

        if pattern.ends_with('/') {
            let dir_prefix = pattern.trim_end_matches('/');
            if rel_str == dir_prefix
                || rel_str.starts_with(pattern)
                || rel_str.starts_with(&format!("{}/", dir_prefix))
                || rel_str.ends_with(&format!("/{}", dir_prefix))
                || rel_str.contains(&format!("/{}/", dir_prefix))
            {
                return true;
            }
        }

        if pattern.contains('/') {
            if rel_str == pattern {
                return true;
            }
            if glob_match(pattern, &rel_str) {
                return true;
            }
            if rel_str.ends_with(&format!("/{}", pattern)) {
                return true;
            }
            if glob_match(&format!("*/{}", pattern), &rel_str) {
                return true;
            }
        } else {
            if file_name == pattern || file_stem == pattern {
                return true;
            }
            if glob_match(pattern, &file_name) {
                return true;
            }
            if glob_match(pattern, &file_stem) {
                return true;
            }
            if glob_match(pattern, &rel_str) {
                return true;
            }
        }
    }
    false
}

/// Checks whether a database catalog object matches any ignore pattern from `.duckpackignore`.
pub fn is_object_ignored(obj_name: &str, rules: &[String]) -> bool {
    if rules.is_empty() {
        return false;
    }
    let unqual_name = obj_name.split('.').last().unwrap_or(obj_name);

    for raw_rule in rules {
        let pattern = raw_rule.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        let pattern = pattern.replace('\\', "/");
        let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);

        if pattern.is_empty() {
            continue;
        }

        if obj_name == pattern || unqual_name == pattern {
            return true;
        }
        if glob_match(pattern, obj_name) || glob_match(pattern, unqual_name) {
            return true;
        }

        if pattern.contains('/') {
            let p = Path::new(pattern);
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if !stem.is_empty()
                    && (stem == obj_name
                        || stem == unqual_name
                        || glob_match(stem, obj_name)
                        || glob_match(stem, unqual_name))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Auto-discovers the active DuckPack project directory.
pub fn discover_project_dir(start: &Path) -> PathBuf {
    if is_project_root(start) {
        return start.to_path_buf();
    }

    let candidates = [
        "duckpack",
        "duckdb_project",
        "duckdb",
        "project",
        "demo_project",
    ];
    for sub in &candidates {
        let sub_path = start.join(sub);
        if sub_path.is_dir() && is_project_root(&sub_path) {
            return sub_path;
        }
    }

    if let Ok(entries) = fs::read_dir(start) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let p = entry.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    if !name.starts_with('.')
                        && !name.starts_with("target")
                        && !name.starts_with("node_modules")
                        && !name.starts_with("venv")
                        && is_project_root(&p)
                    {
                        return p;
                    }
                }
            }
        }
    }

    let mut current = start.parent();
    while let Some(parent) = current {
        if is_project_root(parent) {
            return parent.to_path_buf();
        }
        current = parent.parent();
    }

    start.to_path_buf()
}

pub(crate) fn is_project_root(path: &Path) -> bool {
    path.join("duckpack.toml").exists()
        || path.join("queries").is_dir()
        || path.join("tables").is_dir()
        || path.join("schemas").is_dir()
        || path.join("sequences").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shadow_db_is_sandboxed_against_external_access() {
        let dir = std::env::temp_dir().join(format!(
            "dp_shadow_sandbox_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("tables")).unwrap();
        std::fs::write(
            dir.join("tables").join("evil.sql"),
            "CREATE TABLE evil AS SELECT * FROM read_text('/etc/hostname');",
        )
        .unwrap();

        let err = build_shadow_db(&dir).unwrap_err();
        let chain = format!("{:#}", err);
        assert!(
            chain.to_lowercase().contains("external")
                || chain.to_lowercase().contains("disabled")
                || chain.to_lowercase().contains("permission"),
            "Expected Shadow DB sandbox to block read_text(), got: {chain}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
