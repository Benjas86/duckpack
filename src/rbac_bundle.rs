use anyhow::{Context, Result};
use duckdb::{AccessMode, Config, Connection};
use std::path::Path;

use crate::engine::DbExecutor;
use crate::quack_auth::{
    deploy_quack_rbac_from_config, load_quack_auth_config, validate_safe_username, QuackAuthConfig,
    QuackAuthUser, QuackRole,
};

const RBAC_BUNDLE_VERSION: i32 = 1;

/// Validates a `QuackAuthConfig` structurally (usernames + Admin Lockout Prevention Guard)
/// WITHOUT requiring runtime secrets/tokens to be present in `.env` or OS env vars.
/// This allows CI/CD pipelines to compile a zero-secret `rbac.duckpack` bundle safely.
pub fn validate_quack_auth_structure(config: &QuackAuthConfig) -> Result<()> {
    let mut has_enabled_admin = config.default_role == QuackRole::Admin;
    let mut seen_users = std::collections::HashSet::new();
    let mut seen_token_envs = std::collections::HashSet::new();

    for user in &config.users {
        validate_safe_username(&user.username)?;
        if !seen_users.insert(user.username.to_lowercase()) {
            anyhow::bail!(
                "Duplicate Quack RBAC username '{}' in [[quack_auth.users]].",
                user.username
            );
        }
        let trimmed_env = user.token_env.trim();
        if trimmed_env.is_empty() {
            anyhow::bail!(
                "Quack RBAC user '{}' must specify a non-empty token_env variable name.",
                user.username
            );
        }
        if !seen_token_envs.insert(trimmed_env.to_string()) {
            anyhow::bail!(
                "Duplicate Quack RBAC token_env '{}' across users; each user must have a distinct secret token.",
                trimmed_env
            );
        }
        if !config.allow_insecure_client_id {
            if let Some(ref cid) = user.client_id {
                if !cid.trim().is_empty() {
                    anyhow::bail!(
                        "Security Guard: Quack RBAC user '{}' specifies a plaintext client_id ('{}') in duckpack.toml, which allows any authenticated user to spoof another role by supplying that CLIENT_ID. Omit client_id so Quack binds CLIENT_ID to the user's secret token_env (or set allow_insecure_client_id = true only for isolated legacy setups).",
                        user.username,
                        cid.trim()
                    );
                }
            }
        }
        if user.enabled && user.role == QuackRole::Admin {
            has_enabled_admin = true;
        }
    }

    if config.enabled && !has_enabled_admin {
        anyhow::bail!(
            "Admin Lockout Prevention Guard: Cannot compile Quack RBAC bundle without at least one enabled 'admin' user (or default_role = \"admin\")."
        );
    }

    Ok(())
}

/// Writes the zero-secret portable RBAC policy metadata tables (`_duckpack_rbac_bundle_meta`
/// and `_duckpack_rbac_bundle_users`) into an open `.duckpack` DuckDB connection.
///
/// Strict Zero-Secret Guarantee: Cleartext tokens and SHA-256 token hashes are NEVER written
/// into the bundle — only `token_env` environment variable references are stored.
pub fn write_rbac_bundle_tables(conn: &Connection, config: &QuackAuthConfig) -> Result<usize> {
    validate_quack_auth_structure(config)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _duckpack_rbac_bundle_meta (\n\
         \x20   bundle_version INTEGER NOT NULL,\n\
         \x20   enabled BOOLEAN NOT NULL,\n\
         \x20   ducklake_mode BOOLEAN NOT NULL,\n\
         \x20   default_role VARCHAR NOT NULL,\n\
         \x20   writer_allow_temp_tables BOOLEAN NOT NULL,\n\
         \x20   reader_allow_explain_describe BOOLEAN NOT NULL,\n\
         \x20   allow_copy BOOLEAN NOT NULL,\n\
         \x20   allow_insecure_client_id BOOLEAN NOT NULL DEFAULT false\n\
         );\n\
         CREATE TABLE IF NOT EXISTS _duckpack_rbac_bundle_users (\n\
         \x20   username VARCHAR PRIMARY KEY,\n\
         \x20   role VARCHAR NOT NULL,\n\
         \x20   token_env VARCHAR NOT NULL,\n\
         \x20   client_id VARCHAR,\n\
         \x20   enabled BOOLEAN NOT NULL\n\
         );\n\
         DELETE FROM _duckpack_rbac_bundle_meta;\n\
         DELETE FROM _duckpack_rbac_bundle_users;",
    )
    .with_context(|| "Failed initializing _duckpack_rbac_bundle_* tables")?;

    conn.execute(
        "INSERT INTO _duckpack_rbac_bundle_meta \
         (bundle_version, enabled, ducklake_mode, default_role, writer_allow_temp_tables, reader_allow_explain_describe, allow_copy, allow_insecure_client_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        duckdb::params![
            RBAC_BUNDLE_VERSION,
            config.enabled,
            config.ducklake_mode,
            config.default_role.as_str(),
            config.writer_allow_temp_tables,
            config.reader_allow_explain_describe,
            config.allow_copy,
            config.allow_insecure_client_id,
        ],
    )
    .with_context(|| "Failed writing _duckpack_rbac_bundle_meta")?;

    for user in &config.users {
        conn.execute(
            "INSERT INTO _duckpack_rbac_bundle_users (username, role, token_env, client_id, enabled) \
             VALUES (?, ?, ?, ?, ?)",
            duckdb::params![
                user.username,
                user.role.as_str(),
                user.token_env,
                user.client_id.as_deref(),
                user.enabled,
            ],
        )
        .with_context(|| {
            format!(
                "Failed writing RBAC user '{}' into bundle",
                user.username
            )
        })?;
    }

    Ok(config.users.len())
}

/// Compiles ONLY the declarative `[quack_auth]` RBAC policy from `project_dir/duckpack.toml`
/// into a standalone, zero-secret `.duckpack` bundle at `out_path`.
pub fn compile_rbac_bundle(project_dir: &Path, out_path: &Path) -> Result<usize> {
    let config = load_quack_auth_config(project_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "No [quack_auth] section found in {}/duckpack.toml.",
            project_dir.display()
        )
    })?;
    validate_quack_auth_structure(&config)?;

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let _ = std::fs::remove_file(out_path);
    let out_conn = Connection::open(out_path)
        .with_context(|| format!("Failed to create RBAC bundle '{}'", out_path.display()))?;
    write_rbac_bundle_tables(&out_conn, &config)
}

/// Loads a `QuackAuthConfig` directly from a compiled `.duckpack` RBAC bundle file
/// without requiring any `duckpack.toml` or project directory on disk.
pub fn load_rbac_bundle(bundle_path: &Path) -> Result<QuackAuthConfig> {
    if !bundle_path.exists() {
        anyhow::bail!("RBAC bundle '{}' does not exist.", bundle_path.display());
    }
    let cfg = Config::default().access_mode(AccessMode::ReadOnly)?;
    let conn = Connection::open_with_flags(bundle_path, cfg).with_context(|| {
        format!(
            "Failed to open RBAC bundle '{}' in read-only mode",
            bundle_path.display()
        )
    })?;

    let (
        bundle_version,
        enabled,
        ducklake_mode,
        default_role_str,
        writer_allow_temp_tables,
        reader_allow_explain_describe,
        allow_copy,
    ): (i32, bool, bool, String, bool, bool, bool) = conn
        .query_row(
            "SELECT bundle_version, enabled, ducklake_mode, default_role, \
             writer_allow_temp_tables, reader_allow_explain_describe, allow_copy \
             FROM _duckpack_rbac_bundle_meta LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .with_context(|| {
            format!(
                "'{}' does not contain a valid _duckpack_rbac_bundle_meta table. Build it with `duckpack compile --rbac-only --out <file>`.",
                bundle_path.display()
            )
        })?;

    if bundle_version > RBAC_BUNDLE_VERSION {
        anyhow::bail!(
            "RBAC bundle '{}' uses unsupported schema version {} (max supported: {}). Upgrade duckpack.",
            bundle_path.display(),
            bundle_version,
            RBAC_BUNDLE_VERSION
        );
    }

    let mut stmt = conn.prepare(
        "SELECT username, role, token_env, client_id, enabled \
         FROM _duckpack_rbac_bundle_users ORDER BY username",
    )?;
    let mut rows = stmt.query([])?;
    let mut users = Vec::new();
    while let Some(row) = rows.next()? {
        let username: String = row.get(0)?;
        let role_str: String = row.get(1)?;
        let token_env: String = row.get(2)?;
        let client_id: Option<String> = row.get(3)?;
        let user_enabled: bool = row.get(4)?;
        users.push(QuackAuthUser {
            username,
            role: QuackRole::from_str(&role_str),
            token_env,
            client_id: client_id.filter(|s| !s.trim().is_empty()),
            enabled: user_enabled,
        });
    }

    let allow_insecure_client_id: bool = conn
        .query_row(
            "SELECT allow_insecure_client_id FROM _duckpack_rbac_bundle_meta LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);

    Ok(QuackAuthConfig {
        enabled,
        ducklake_mode,
        default_role: QuackRole::from_str(&default_role_str),
        writer_allow_temp_tables,
        reader_allow_explain_describe,
        allow_copy,
        allow_insecure_client_id,
        users,
    })
}

/// Loads runtime environment variables from an explicit `--env-file` path (e.g. a Kubernetes
/// mounted secret file) before resolving RBAC tokens.
pub fn load_env_file_override(env_file: &Path) -> Result<()> {
    if !env_file.exists() {
        anyhow::bail!("Specified --env-file '{}' does not exist.", env_file.display());
    }
    dotenvy::from_path_override(env_file).with_context(|| {
        format!(
            "Failed to parse environment variables from '{}'",
            env_file.display()
        )
    })?;
    Ok(())
}

/// Applies a portable `.duckpack` RBAC bundle directly to `exec` with ZERO project directory
/// required: resolves tokens from container OS env vars (and optional `--env-file`), harvests
/// live server-salted `client_id_hash` values, commits `_duckpack_auth`, and activates global hooks.
pub fn deploy_quack_rbac_from_bundle(
    exec: &mut dyn DbExecutor,
    quack_alias: Option<&str>,
    bundle_path: &Path,
    env_file: Option<&Path>,
) -> Result<usize> {
    if let Some(ef) = env_file {
        load_env_file_override(ef)?;
    }
    let config = load_rbac_bundle(bundle_path)?;
    deploy_quack_rbac_from_config(exec, quack_alias, &config, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quack_auth::save_quack_auth_config;

    #[test]
    fn test_rbac_bundle_zero_secret_compile_load_and_deploy_without_project_dir() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_rbac_bundle_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let proj_dir = temp_dir.join("project");
        std::fs::create_dir_all(&proj_dir).unwrap();

        let cfg = QuackAuthConfig {
            enabled: true,
            ducklake_mode: true,
            default_role: QuackRole::Reader,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
            allow_insecure_client_id: false,
            users: vec![
                QuackAuthUser {
                    username: "lake_admin".to_string(),
                    role: QuackRole::Admin,
                    token_env: "DP_TEST_BUNDLE_ADMIN_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "etl_writer".to_string(),
                    role: QuackRole::Writer,
                    token_env: "DP_TEST_BUNDLE_WRITER_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
            ],
        };
        save_quack_auth_config(&proj_dir, &cfg).unwrap();

        // 1. Compile bundle with ZERO tokens set in env or .env (proves CI zero-secret build works!)
        unsafe {
            std::env::remove_var("DP_TEST_BUNDLE_ADMIN_TOK");
            std::env::remove_var("DP_TEST_BUNDLE_WRITER_TOK");
        }
        let bundle_path = temp_dir.join("rbac.duckpack");
        let count = compile_rbac_bundle(&proj_dir, &bundle_path).unwrap();
        assert_eq!(count, 2);

        // 2. Delete the entire project directory to prove zero-project-dir portability!
        std::fs::remove_dir_all(&proj_dir).unwrap();
        assert!(!proj_dir.exists());

        // 3. Verify round-trip config load and zero-secret contents inside bundle
        let loaded = load_rbac_bundle(&bundle_path).unwrap();
        let mut expected = cfg.clone();
        expected.users.sort_by(|a, b| a.username.cmp(&b.username));
        assert_eq!(loaded, expected);
        assert!(loaded.ducklake_mode);

        // Inspect bundle schema to verify no token_sha256 / secret column exists
        let inspect_conn = Connection::open(&bundle_path).unwrap();
        let cols: Vec<String> = inspect_conn
            .prepare("SELECT column_name FROM duckdb_columns() WHERE table_name = '_duckpack_rbac_bundle_users'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .flatten()
            .collect();
        assert!(cols.contains(&"token_env".to_string()));
        assert!(!cols.iter().any(|c| c.contains("sha256") || c == "token"));
        drop(inspect_conn);

        // 4. Deploy from bundle using an explicit --env-file (simulating mounted container secret)
        let runtime_env = temp_dir.join("runtime.env");
        std::fs::write(
            &runtime_env,
            "DP_TEST_BUNDLE_ADMIN_TOK=super_admin_runtime_tok_999\nDP_TEST_BUNDLE_WRITER_TOK=etl_writer_runtime_tok_888\n",
        )
        .unwrap();

        let target_conn = Connection::open_in_memory().unwrap();
        let mut exec: &Connection = &target_conn;
        let deployed =
            deploy_quack_rbac_from_bundle(&mut exec, None, &bundle_path, Some(&runtime_env))
                .unwrap();
        assert_eq!(deployed, 2);

        // Verify DuckLake-mode RBAC macros are active in target_conn!
        target_conn
            .execute_batch(
                "INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) VALUES ('sid_w', 'etl_writer', 'writer', false);",
            )
            .unwrap();
        let writer_can_dml: bool = target_conn
            .query_row(
                "SELECT _duckpack_auth.authorize('sid_w', 'INSERT INTO ducklake_data_file VALUES (1, 2);');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let writer_blocked_ddl: bool = target_conn
            .query_row(
                "SELECT _duckpack_auth.authorize('sid_w', 'INSERT INTO ducklake_table VALUES (1, 2);');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(writer_can_dml, "Writer must be allowed DuckLake data_file DML");
        assert!(
            !writer_blocked_ddl,
            "Writer must be blocked from DuckLake ducklake_table DDL when ducklake_mode = true"
        );

        unsafe {
            std::env::remove_var("DP_TEST_BUNDLE_ADMIN_TOK");
            std::env::remove_var("DP_TEST_BUNDLE_WRITER_TOK");
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
