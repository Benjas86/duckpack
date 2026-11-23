use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::diff::{DiffItem, DiffItemType, DiffResult};
use crate::engine::DbExecutor;

/// Built-in RBAC role presets for Quack server sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuackRole {
    Admin,
    Writer,
    Reader,
    None,
}

impl QuackRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuackRole::Admin => "admin",
            QuackRole::Writer => "writer",
            QuackRole::Reader => "reader",
            QuackRole::None => "none",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "admin" => QuackRole::Admin,
            "writer" => QuackRole::Writer,
            "reader" => QuackRole::Reader,
            "none" | "disabled" | "revoked" => QuackRole::None,
            // Fail closed on unknown/typoed role strings rather than granting read access.
            _ => QuackRole::None,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            QuackRole::Admin => QuackRole::Writer,
            QuackRole::Writer => QuackRole::Reader,
            QuackRole::Reader => QuackRole::Admin,
            QuackRole::None => QuackRole::Reader,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            QuackRole::Admin => QuackRole::Reader,
            QuackRole::Writer => QuackRole::Admin,
            QuackRole::Reader => QuackRole::Writer,
            QuackRole::None => QuackRole::Reader,
        }
    }
}

/// Represents a declarative Quack user mapping in `[[quack_auth.users]]` inside `duckpack.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuackAuthUser {
    pub username: String,
    pub role: QuackRole,
    pub token_env: String,
    pub client_id: Option<String>,
    pub enabled: bool,
}

/// Represents the `[quack_auth]` configuration block in `duckpack.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuackAuthConfig {
    pub enabled: bool,
    pub ducklake_mode: bool,
    pub default_role: QuackRole,
    pub writer_allow_temp_tables: bool,
    pub reader_allow_explain_describe: bool,
    pub allow_copy: bool,
    /// Opt-in escape hatch for legacy plaintext `client_id` values in `duckpack.toml`.
    /// Defaults to `false` (secure): `client_id` binds automatically to each user's secret
    /// `token_env` so low-privilege users cannot spoof an admin's `CLIENT_ID`.
    pub allow_insecure_client_id: bool,
    pub users: Vec<QuackAuthUser>,
}

impl Default for QuackAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ducklake_mode: false,
            default_role: QuackRole::Reader,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
            allow_insecure_client_id: false,
            users: Vec::new(),
        }
    }
}

/// Pure-Rust FIPS 180-4 SHA-256 implementation returning a 64-character lowercase hex digest.
/// Matches DuckDB's built-in `sha256(VARCHAR)` function output byte-for-byte.
pub fn sha256_hex(input: &str) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bytes = input.as_bytes();
    let bit_len = (bytes.len() as u64) * 8;
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    format!(
        "{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
        h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]
    )
}

/// Validates that a username or env var identifier contains only safe characters.
pub fn validate_safe_username(username: &str) -> Result<()> {
    let trimmed = username.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Quack RBAC username cannot be empty.");
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        anyhow::bail!(
            "Invalid Quack RBAC username '{}': only alphanumeric characters, '_', '-', and '.' are permitted.",
            username
        );
    }
    Ok(())
}

/// Loads the `[quack_auth]` configuration block from `duckpack.toml` in `project_dir`, if present.
pub fn load_quack_auth_config(project_dir: &Path) -> Option<QuackAuthConfig> {
    let toml_path = project_dir.join("duckpack.toml");
    let content = std::fs::read_to_string(toml_path).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;
    let qa = parsed.get("quack_auth")?;

    let enabled = qa.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let ducklake_mode = qa
        .get("ducklake_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let default_role = qa
        .get("default_role")
        .and_then(|v| v.as_str())
        .map(QuackRole::from_str)
        .unwrap_or(QuackRole::Reader);
    let writer_allow_temp_tables = qa
        .get("writer_allow_temp_tables")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let reader_allow_explain_describe = qa
        .get("reader_allow_explain_describe")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let allow_copy = qa
        .get("allow_copy")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let allow_insecure_client_id = qa
        .get("allow_insecure_client_id")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut users = Vec::new();
    if let Some(arr) = qa.get("users").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(username) = item.get("username").and_then(|v| v.as_str()) {
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .map(QuackRole::from_str)
                    .unwrap_or(QuackRole::Reader);
                let default_env = format!(
                    "QUACK_TOKEN_{}",
                    username
                        .trim()
                        .to_uppercase()
                        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
                );
                let token_env = item
                    .get("token_env")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(default_env);
                let client_id = item
                    .get("client_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                let user_enabled = item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                users.push(QuackAuthUser {
                    username: username.trim().to_string(),
                    role,
                    token_env,
                    client_id,
                    enabled: user_enabled,
                });
            }
        }
    }

    Some(QuackAuthConfig {
        enabled,
        ducklake_mode,
        default_role,
        writer_allow_temp_tables,
        reader_allow_explain_describe,
        allow_copy,
        allow_insecure_client_id,
        users,
    })
}

/// Resolves a user's cleartext token from process environment variables or an optional `project_dir/.env`.
pub fn resolve_user_token_opt(project_dir: Option<&Path>, token_env: &str) -> Option<String> {
    crate::env_store::resolve_env_var(project_dir, None, token_env)
}

/// Resolves a user's cleartext token from process environment variables or `project_dir/.env`.
pub fn resolve_user_token(project_dir: &Path, token_env: &str) -> Option<String> {
    resolve_user_token_opt(Some(project_dir), token_env)
}

/// Generates a 256-bit cryptographic random token and persists `<token_env>=<token>` in `project_dir/.env`
/// with `0600` permissions on Unix.
pub fn generate_and_save_user_token(project_dir: &Path, token_env: &str) -> Result<String> {
    let token = crate::env_store::generate_random_hex_token();
    save_env_key_value(project_dir, token_env, &token)?;
    Ok(token)
}

/// Upserts `<key>=<value>` into `project_dir/.env` atomically with `0600` permissions on Unix
/// and newline/control-character sanitization to prevent `.env` line injection.
pub fn save_env_key_value(project_dir: &Path, key: &str, value: &str) -> Result<()> {
    let clean_key = crate::env_store::sanitize_env_value(key.trim());
    if clean_key.is_empty() || clean_key.contains('=') {
        anyhow::bail!("Invalid environment variable name '{}'.", key);
    }
    crate::env_store::upsert_env_pairs(project_dir, None, &[(&clean_key, value)])?;
    Ok(())
}

/// Saves or updates the `[quack_auth]` and `[[quack_auth.users]]` section in `project_dir/duckpack.toml`.
pub fn save_quack_auth_config(project_dir: &Path, config: &QuackAuthConfig) -> Result<()> {
    let toml_path = project_dir.join("duckpack.toml");
    let existing = std::fs::read_to_string(&toml_path).unwrap_or_default();
    let mut root: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(&existing).unwrap_or_else(|_| toml::Table::new())
    };

    let mut qa_table = toml::map::Map::new();
    qa_table.insert("enabled".to_string(), toml::Value::Boolean(config.enabled));
    if config.ducklake_mode {
        qa_table.insert(
            "ducklake_mode".to_string(),
            toml::Value::Boolean(config.ducklake_mode),
        );
    }
    qa_table.insert(
        "default_role".to_string(),
        toml::Value::String(config.default_role.as_str().to_string()),
    );
    qa_table.insert(
        "writer_allow_temp_tables".to_string(),
        toml::Value::Boolean(config.writer_allow_temp_tables),
    );
    qa_table.insert(
        "reader_allow_explain_describe".to_string(),
        toml::Value::Boolean(config.reader_allow_explain_describe),
    );
    qa_table.insert(
        "allow_copy".to_string(),
        toml::Value::Boolean(config.allow_copy),
    );
    if config.allow_insecure_client_id {
        qa_table.insert(
            "allow_insecure_client_id".to_string(),
            toml::Value::Boolean(config.allow_insecure_client_id),
        );
    }

    let mut users_arr = Vec::new();
    for u in &config.users {
        let mut ut = toml::map::Map::new();
        ut.insert(
            "username".to_string(),
            toml::Value::String(u.username.clone()),
        );
        ut.insert(
            "role".to_string(),
            toml::Value::String(u.role.as_str().to_string()),
        );
        ut.insert(
            "token_env".to_string(),
            toml::Value::String(u.token_env.clone()),
        );
        if let Some(ref cid) = u.client_id {
            if !cid.trim().is_empty() {
                ut.insert(
                    "client_id".to_string(),
                    toml::Value::String(cid.trim().to_string()),
                );
            }
        }
        ut.insert("enabled".to_string(), toml::Value::Boolean(u.enabled));
        users_arr.push(toml::Value::Table(ut));
    }
    qa_table.insert("users".to_string(), toml::Value::Array(users_arr));
    root.insert("quack_auth".to_string(), toml::Value::Table(qa_table));

    let serialized = toml::to_string_pretty(&root)
        .with_context(|| "Failed to serialize duckpack.toml with [quack_auth]")?;
    std::fs::write(&toml_path, serialized)
        .with_context(|| format!("Failed to write {}", toml_path.display()))?;
    Ok(())
}

/// Validates a `QuackAuthConfig` before compiling or deploying to a server, enforcing the
/// Admin Lockout Prevention Guard (at least one enabled `admin` user with a resolved token).
/// When `project_dir` is `None` (e.g. deploying from a portable `.duckpack` RBAC bundle),
/// tokens are resolved solely from process environment variables.
pub fn validate_quack_auth_config_opt(
    config: &QuackAuthConfig,
    project_dir: Option<&Path>,
) -> Result<Vec<(QuackAuthUser, String, String)>> {
    let mut resolved_users = Vec::new();
    let mut has_enabled_admin = config.default_role == QuackRole::Admin;
    let mut seen_token_hashes = std::collections::HashSet::new();
    let mut seen_client_id_hashes = std::collections::HashSet::new();

    for user in &config.users {
        validate_safe_username(&user.username)?;
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
        let token = resolve_user_token_opt(project_dir, &user.token_env).ok_or_else(|| {
            anyhow::anyhow!(
                "Quack RBAC user '{}' references token_env '{}', which is not set in environment or .env.",
                user.username,
                user.token_env
            )
        })?;
        let token_hash = sha256_hex(&token);
        if !seen_token_hashes.insert(token_hash.clone()) {
            anyhow::bail!(
                "Security Guard: Quack RBAC user '{}' shares a duplicate secret token with another user. Each user must have a unique token.",
                user.username
            );
        }
        let raw_client_id = user.client_id.as_deref().unwrap_or(&token);
        let client_id_hash = sha256_hex(raw_client_id);
        if !seen_client_id_hashes.insert(client_id_hash.clone()) {
            anyhow::bail!(
                "Security Guard: Quack RBAC user '{}' shares a duplicate client_id with another user. Each user must have a unique client_id.",
                user.username
            );
        }

        if user.enabled && user.role == QuackRole::Admin {
            has_enabled_admin = true;
        }
        resolved_users.push((user.clone(), token_hash, client_id_hash));
    }

    if config.enabled && !has_enabled_admin {
        anyhow::bail!(
            "Admin Lockout Prevention Guard: Cannot enable Quack RBAC without at least one enabled 'admin' user in [[quack_auth.users]] (or default_role = \"admin\")."
        );
    }

    Ok(resolved_users)
}

/// Validates a `QuackAuthConfig` against `project_dir` (and OS env vars).
pub fn validate_quack_auth_config(
    config: &QuackAuthConfig,
    project_dir: &Path,
) -> Result<Vec<(QuackAuthUser, String, String)>> {
    validate_quack_auth_config_opt(config, Some(project_dir))
}

/// Compiles the idempotent `_duckpack_auth` schema, tables, user upserts, and hardened
/// `_duckpack_auth.authenticate` + `_duckpack_auth.authorize` SQL macros.
/// If `deploying_sid` is provided, automatically registers that session ID as `admin` in
/// `_duckpack_auth.quack_sessions` so the deploying connection cannot lock itself out.
pub fn compile_quack_auth_sql(
    config: &QuackAuthConfig,
    project_dir: &Path,
    deploying_sid: Option<&str>,
    include_global_hooks: bool,
) -> Result<String> {
    let empty_hashes = std::collections::HashMap::new();
    compile_quack_auth_sql_with_options(
        config,
        project_dir,
        deploying_sid,
        None,
        &empty_hashes,
        include_global_hooks,
        include_global_hooks,
    )
}

/// Full compiler for `_duckpack_auth` with optional `project_dir` (pass `None` for zero-project-dir
/// `.duckpack` RBAC bundle deployments).
pub fn compile_quack_auth_sql_with_options_opt(
    config: &QuackAuthConfig,
    project_dir: Option<&Path>,
    deploying_sid: Option<&str>,
    master_client_id_hash: Option<&str>,
    live_client_id_hashes: &std::collections::HashMap<String, String>,
    quack_server_mode: bool,
    include_global_hooks: bool,
) -> Result<String> {
    let resolved_users = validate_quack_auth_config_opt(config, project_dir)?;

    let mut sql = String::new();
    sql.push_str("-- DuckPack Declarative Quack RBAC (_duckpack_auth)\n");
    sql.push_str("CREATE SCHEMA IF NOT EXISTS _duckpack_auth;\n");
    sql.push_str(
        "CREATE TABLE IF NOT EXISTS _duckpack_auth.quack_users (\n\
         \x20   username VARCHAR PRIMARY KEY,\n\
         \x20   role VARCHAR NOT NULL,\n\
         \x20   token_sha256 VARCHAR NOT NULL,\n\
         \x20   client_id_hash VARCHAR NOT NULL,\n\
         \x20   enabled BOOLEAN NOT NULL DEFAULT true\n\
         );\n",
    );
    sql.push_str(
        "CREATE TABLE IF NOT EXISTS _duckpack_auth.quack_sessions (\n\
         \x20   sid VARCHAR PRIMARY KEY,\n\
         \x20   username VARCHAR,\n\
         \x20   role VARCHAR NOT NULL,\n\
         \x20   revoked BOOLEAN NOT NULL DEFAULT false,\n\
         \x20   updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP\n\
         );\n",
    );

    // Declaratively sync _duckpack_auth.quack_users to match duckpack.toml
    sql.push_str("DELETE FROM _duckpack_auth.quack_users;\n");
    for (u, token_hash, default_client_id_hash) in &resolved_users {
        let effective_cid_hash = live_client_id_hashes
            .get(&u.username)
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| default_client_id_hash.to_lowercase());
        let safe_user = u.username.replace('\'', "''");
        let safe_role = u.role.as_str().replace('\'', "''");
        let safe_tok_hash = token_hash.replace('\'', "''");
        let safe_cid_hash = effective_cid_hash.replace('\'', "''");
        sql.push_str(&format!(
            "INSERT INTO _duckpack_auth.quack_users (username, role, token_sha256, client_id_hash, enabled) \
             VALUES ('{}', '{}', '{}', '{}', {});\n",
            safe_user, safe_role, safe_tok_hash, safe_cid_hash, u.enabled
        ));
    }

    // Bind the deploying master connection's server-salted client_id_hash to 'admin'
    // (empty token_sha256 so it never grants authentication on its own; only grants admin role
    // to connections authenticated by quack_check_token or matching client_id_hash).
    if let Some(mh) = master_client_id_hash
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let safe_mh = mh.to_lowercase().replace('\'', "''");
        sql.push_str(&format!(
            "INSERT INTO _duckpack_auth.quack_users (username, role, token_sha256, client_id_hash, enabled) \
             VALUES ('__duckpack_master', 'admin', '', '{}', true);\n",
            safe_mh
        ));
    }

    // Auto-whitelist deploying session as 'admin' to prevent self-lockout
    if let Some(sid) = deploying_sid.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let safe_sid = sid.replace('\'', "''");
        sql.push_str(&format!(
            "DELETE FROM _duckpack_auth.quack_sessions WHERE lower(CAST(sid AS VARCHAR)) = lower('{}');\n\
             INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) \
             VALUES ('{}', '__duckpack_deployer', 'admin', false);\n",
            safe_sid, safe_sid
        ));
    }

    // 1. Authentication Macro: _duckpack_auth.authenticate(p_sid, p_client_token, p_server_token)
    // Allows:
    //  - Any enabled user in _duckpack_auth.quack_users matching sha256(p_client_token)
    //  - Break-glass fallback for the server's own startup master token via quack_check_token()
    // Always rejects any session explicitly marked revoked = true in _duckpack_auth.quack_sessions.
    let master_token_fallback = if quack_server_mode {
        "        WHEN quack_check_token(p_sid, p_client_token, p_server_token) THEN true\n"
    } else {
        "        WHEN p_server_token IS NOT NULL AND p_server_token <> '' AND sha256(COALESCE(p_client_token, '')) = sha256(p_server_token) THEN true\n"
    };
    sql.push_str(&format!(
        "CREATE OR REPLACE MACRO _duckpack_auth.authenticate(p_sid, p_client_token, p_server_token) AS (\n\
         \x20   CASE\n\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_sessions s\n\
         \x20           WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(p_sid AS VARCHAR)) AND s.revoked = true\n\
         \x20       ) THEN false\n\
         {master_token_fallback}\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_users u\n\
         \x20           WHERE u.enabled = true\n\
         \x20             AND u.token_sha256 <> ''\n\
         \x20             AND u.token_sha256 = sha256(COALESCE(p_client_token, ''))\n\
         \x20       ) THEN true\n\
         \x20       ELSE false\n\
         \x20   END\n\
         );\n"
    ));

    // 2. Role Resolution Macro: _duckpack_auth.resolve_role(p_sid)
    // Correlates p_sid via:
    //  1) Explicit session override / revocation in _duckpack_auth.quack_sessions
    //  2) Live Quack connection table quack_active_connections() (connection_id = p_sid)
    //     matched against _duckpack_auth.quack_users.client_id_hash / token_sha256 (least-privilege tie-break)
    //  3) Direct client_id_hash / token_sha256 match (and username only in local non-server mode)
    //  4) Configured default_role fallback
    let default_role_str = config.default_role.as_str();
    let quack_active_conn_clause = if quack_server_mode {
        "            (SELECT u.role FROM quack_active_connections() ac JOIN _duckpack_auth.quack_users u ON u.enabled = true AND (lower(u.client_id_hash) = lower(CAST(ac.client_id_hash AS VARCHAR)) OR (u.token_sha256 <> '' AND lower(u.token_sha256) = lower(CAST(ac.client_id_hash AS VARCHAR)))) WHERE lower(CAST(ac.connection_id AS VARCHAR)) = lower(CAST(p_sid AS VARCHAR)) ORDER BY CASE u.role WHEN 'none' THEN 1 WHEN 'reader' THEN 2 WHEN 'writer' THEN 3 WHEN 'admin' THEN 4 ELSE 1 END LIMIT 1),\n"
    } else {
        ""
    };
    let direct_user_match_clause = if quack_server_mode {
        // In Quack server mode, never match p_sid against raw usernames (prevents role-enumeration oracles).
        "            (SELECT u.role FROM _duckpack_auth.quack_users u WHERE u.enabled = true AND (lower(u.client_id_hash) = lower(CAST(p_sid AS VARCHAR)) OR (u.token_sha256 <> '' AND lower(u.token_sha256) = sha256(COALESCE(CAST(p_sid AS VARCHAR), '')))) ORDER BY CASE u.role WHEN 'none' THEN 1 WHEN 'reader' THEN 2 WHEN 'writer' THEN 3 WHEN 'admin' THEN 4 ELSE 1 END LIMIT 1),\n"
    } else {
        "            (SELECT u.role FROM _duckpack_auth.quack_users u WHERE u.enabled = true AND (u.username = CAST(p_sid AS VARCHAR) OR lower(u.client_id_hash) = lower(CAST(p_sid AS VARCHAR)) OR (u.token_sha256 <> '' AND lower(u.token_sha256) = sha256(COALESCE(CAST(p_sid AS VARCHAR), '')))) ORDER BY CASE u.role WHEN 'none' THEN 1 WHEN 'reader' THEN 2 WHEN 'writer' THEN 3 WHEN 'admin' THEN 4 ELSE 1 END LIMIT 1),\n"
    };
    sql.push_str(&format!(
        "CREATE OR REPLACE MACRO _duckpack_auth.resolve_role(p_sid) AS (\n\
         \x20   CASE\n\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_sessions s\n\
         \x20           WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(p_sid AS VARCHAR)) AND s.revoked = true\n\
         \x20       ) THEN 'none'\n\
         \x20       ELSE COALESCE(\n\
         \x20           (SELECT s.role FROM _duckpack_auth.quack_sessions s WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(p_sid AS VARCHAR)) AND s.revoked = false LIMIT 1),\n\
         {quack_active_conn_clause}\
         {direct_user_match_clause}\
         \x20           '{}'\n\
         \x20       )\n\
         \x20   END\n\
         );\n",
        default_role_str
    ));

    // 3. Single-Pass Lexical SQL Sanitizer Macro: _duckpack_auth.sanitize_sql(p_q)
    // Tokenizes SQL left-to-right in a single recursive pass matching DuckDB v2.0 lexical precedence:
    //  - Single-quoted strings ('...' with '' escapes) -> ''
    //  - Escape strings (e'...' / E'...' with \' and \\ and '' escapes) -> ''
    //  - Tagged & untagged dollar-quoted strings ($tag$...$tag$, $$...$$) -> ''
    //  - Double-quoted identifiers ("..." with "" escapes) -> inner non-[a-z0-9_] sanitized to '_'
    //  - Line comments (--... to \r or \n) -> ' '
    //  - Depth-counted nested block comments (/* ... /* ... */ ... */) -> ' '
    // Fails CLOSED (returns '') on any unclosed string literal, identifier, dollar quote, or comment.
    sql.push_str(
        "CREATE OR REPLACE MACRO _duckpack_auth.sanitize_sql(p_q) AS (\n\
         \x20   (\n\
         \x20       WITH RECURSIVE _lex(rem, out_sql, mode, depth) AS (\n\
         \x20           SELECT COALESCE(p_q, ''), '', 'normal', 0\n\
         \x20           UNION ALL\n\
         \x20           SELECT\n\
         \x20               CASE\n\
         \x20                   WHEN mode = 'normal' AND rem = '' THEN ''\n\
         \x20                   WHEN mode = 'normal' AND length(regexp_extract(rem, '^[^''\"$/-]+')) > 0\n\
         \x20                       THEN substr(rem, length(regexp_extract(rem, '^[^''\"$/-]+')) + 1)\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '--')\n\
         \x20                       THEN substr(rem, length(regexp_extract(rem, '^--[^\\r\\n]*')) + 1)\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '/*')\n\
         \x20                       THEN substr(rem, 3)\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '''')\n\
         \x20                       THEN substr(rem, 2)\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '\"')\n\
         \x20                       THEN substr(rem, 2)\n\
         \x20                   WHEN mode = 'normal' AND regexp_matches(rem, '^\\$[A-Za-z0-9_]*\\$')\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(substr(rem, length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + 1), regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) = 0 THEN ''\n\
         \x20                           ELSE substr(\n\
         \x20                               substr(rem, length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + 1),\n\
         \x20                               strpos(substr(rem, length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + 1), regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$'))\n\
         \x20                           )\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'normal'\n\
         \x20                       THEN substr(rem, 2)\n\
         \x20                   WHEN mode = 'squote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '''') = 0 THEN ''\n\
         \x20                           WHEN substr(rem, strpos(rem, ''''), 2) = '''''' THEN substr(rem, strpos(rem, '''') + 2)\n\
         \x20                           ELSE substr(rem, strpos(rem, '''') + 1)\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'esquote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '''') = 0 THEN ''\n\
         \x20                           WHEN strpos(rem, '\\') > 0 AND strpos(rem, '\\') < strpos(rem, '''')\n\
         \x20                               THEN CASE WHEN strpos(rem, '\\') >= length(rem) THEN '' ELSE substr(rem, strpos(rem, '\\') + 2) END\n\
         \x20                           WHEN substr(rem, strpos(rem, ''''), 2) = '''''' THEN substr(rem, strpos(rem, '''') + 2)\n\
         \x20                           ELSE substr(rem, strpos(rem, '''') + 1)\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'dquote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '\"') = 0 THEN ''\n\
         \x20                           WHEN substr(rem, strpos(rem, '\"'), 2) = '\"\"' THEN substr(rem, strpos(rem, '\"') + 2)\n\
         \x20                           ELSE substr(rem, strpos(rem, '\"') + 1)\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'block'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '*/') = 0 THEN ''\n\
         \x20                           WHEN strpos(rem, '/*') > 0 AND strpos(rem, '/*') < strpos(rem, '*/') THEN substr(rem, strpos(rem, '/*') + 2)\n\
         \x20                           ELSE substr(rem, strpos(rem, '*/') + 2)\n\
         \x20                       END\n\
         \x20                   ELSE ''\n\
         \x20               END,\n\
         \x20               CASE\n\
         \x20                   WHEN mode = 'normal' AND rem = '' THEN out_sql\n\
         \x20                   WHEN mode = 'normal' AND length(regexp_extract(rem, '^[^''\"$/-]+')) > 0\n\
         \x20                       THEN out_sql || lower(regexp_extract(rem, '^[^''\"$/-]+'))\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '--')\n\
         \x20                       THEN out_sql || ' '\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '/*')\n\
         \x20                       THEN out_sql\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '''')\n\
         \x20                       THEN out_sql || ''''''\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '\"')\n\
         \x20                       THEN out_sql || '\"'\n\
         \x20                   WHEN mode = 'normal' AND regexp_matches(rem, '^\\$[A-Za-z0-9_]*\\$')\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(substr(rem, length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + 1), regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) = 0 THEN ''\n\
         \x20                           ELSE out_sql || ''''''\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'normal'\n\
         \x20                       THEN out_sql || substr(rem, 1, 1)\n\
         \x20                   WHEN mode = 'squote' OR mode = 'esquote'\n\
         \x20                       THEN out_sql\n\
         \x20                   WHEN mode = 'dquote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '\"') = 0 THEN ''\n\
         \x20                           WHEN substr(rem, strpos(rem, '\"'), 2) = '\"\"'\n\
         \x20                               THEN out_sql || regexp_replace(lower(substr(rem, 1, strpos(rem, '\"') - 1)), '[^a-z0-9_]', '_', 'g') || '_'\n\
         \x20                           ELSE out_sql || regexp_replace(lower(substr(rem, 1, strpos(rem, '\"') - 1)), '[^a-z0-9_]', '_', 'g') || '\"'\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'block'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '*/') = 0 THEN ''\n\
         \x20                           WHEN strpos(rem, '/*') > 0 AND strpos(rem, '/*') < strpos(rem, '*/') THEN out_sql\n\
         \x20                           WHEN depth <= 1 THEN out_sql || ' '\n\
         \x20                           ELSE out_sql\n\
         \x20                       END\n\
         \x20                   ELSE ''\n\
         \x20               END,\n\
         \x20               CASE\n\
         \x20                   WHEN mode = 'normal' AND rem = '' THEN 'done'\n\
         \x20                   WHEN mode = 'normal' AND length(regexp_extract(rem, '^[^''\"$/-]+')) > 0 THEN 'normal'\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '--') THEN 'normal'\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '/*') THEN 'block'\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '''')\n\
         \x20                       THEN CASE WHEN regexp_matches(out_sql, '(^|[^a-z0-9_])e$') THEN 'esquote' ELSE 'squote' END\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '\"') THEN 'dquote'\n\
         \x20                   WHEN mode = 'normal' AND regexp_matches(rem, '^\\$[A-Za-z0-9_]*\\$')\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(substr(rem, length(regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) + 1), regexp_extract(rem, '^\\$[A-Za-z0-9_]*\\$')) = 0 THEN 'err'\n\
         \x20                           ELSE 'normal'\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'normal' THEN 'normal'\n\
         \x20                   WHEN mode = 'squote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '''') = 0 THEN 'err'\n\
         \x20                           WHEN substr(rem, strpos(rem, ''''), 2) = '''''' THEN 'squote'\n\
         \x20                           ELSE 'normal'\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'esquote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '''') = 0 THEN 'err'\n\
         \x20                           WHEN strpos(rem, '\\') > 0 AND strpos(rem, '\\') < strpos(rem, '''')\n\
         \x20                               THEN CASE WHEN strpos(rem, '\\') >= length(rem) THEN 'err' ELSE 'esquote' END\n\
         \x20                           WHEN substr(rem, strpos(rem, ''''), 2) = '''''' THEN 'esquote'\n\
         \x20                           ELSE 'normal'\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'dquote'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '\"') = 0 THEN 'err'\n\
         \x20                           WHEN substr(rem, strpos(rem, '\"'), 2) = '\"\"' THEN 'dquote'\n\
         \x20                           ELSE 'normal'\n\
         \x20                       END\n\
         \x20                   WHEN mode = 'block'\n\
         \x20                       THEN CASE\n\
         \x20                           WHEN strpos(rem, '*/') = 0 THEN 'err'\n\
         \x20                           WHEN strpos(rem, '/*') > 0 AND strpos(rem, '/*') < strpos(rem, '*/') THEN 'block'\n\
         \x20                           WHEN depth <= 1 THEN 'normal'\n\
         \x20                           ELSE 'block'\n\
         \x20                       END\n\
         \x20                   ELSE 'err'\n\
         \x20               END,\n\
         \x20               CASE\n\
         \x20                   WHEN mode = 'normal' AND starts_with(rem, '/*') THEN 1\n\
         \x20                   WHEN mode = 'block' AND strpos(rem, '*/') > 0 AND strpos(rem, '/*') > 0 AND strpos(rem, '/*') < strpos(rem, '*/') THEN depth + 1\n\
         \x20                   WHEN mode = 'block' AND strpos(rem, '*/') > 0 AND depth > 1 THEN depth - 1\n\
         \x20                   ELSE 0\n\
         \x20               END\n\
         \x20           FROM _lex\n\
         \x20           WHERE mode NOT IN ('done', 'err')\n\
         \x20       )\n\
         \x20       SELECT CASE\n\
         \x20           WHEN mode = 'err' THEN ''\n\
         \x20           ELSE regexp_replace(regexp_replace(out_sql, ';\\s*$', ''), '^\\s+|\\s+$', '', 'g')\n\
         \x20       END\n\
         \x20       FROM _lex\n\
         \x20       WHERE mode IN ('done', 'err')\n\
         \x20       LIMIT 1\n\
         \x20   )\n\
         );\n",
    );

    // 4. Hardened Authorization Macros:
    //    - `_duckpack_auth.authorize_sanitized(p_role, p_sql)` evaluates rules against an already-lexed
    //      `VARCHAR` string `p_sql` (so `sanitize_sql` runs at most ONCE per query and lambdas contain
    //      zero subqueries).
    //    - `_duckpack_auth.authorize(p_sid, p_query)` short-circuits `admin` immediately without running
    //      the lexer, and binds `sanitize_sql(p_query)` once via a single derived table for non-admins.
    const DUCKLAKE_INLINED_CREATE_RE: &str = "^create\\s+table\\s+(if\\s+not\\s+exists\\s+)?([a-z0-9_\"]+\\s*\\.\\s*)*(\"ducklake_inlined_(data|delete)_[a-z0-9_]+\"|ducklake_inlined_(data|delete)_[a-z0-9_]+)\\s*\\(";
    const TEMP_CREATE_RE: &str = "^create\\s+(or\\s+replace\\s+)?(temp|temporary)\\s+(table|view)\\b";

    let writer_prefix_pattern = if config.allow_copy {
        "^(select|with|values|from|table|explain|describe|desc|show|summarize|use|begin|start|commit|rollback|abort|insert|update|delete|merge|create|copy)\\b"
    } else {
        "^(select|with|values|from|table|explain|describe|desc|show|summarize|use|begin|start|commit|rollback|abort|insert|update|delete|merge|create)\\b"
    };

    let writer_batch_and_create_rule = if config.ducklake_mode {
        let per_stmt_create_ok = if config.writer_allow_temp_tables {
            format!(
                "(NOT regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '\\bcreate\\b') \
                  OR regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '{TEMP_CREATE_RE}') \
                  OR (regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '{DUCKLAKE_INLINED_CREATE_RE}') \
                      AND NOT regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '\\bas\\s+(select|with|values|from|table)\\b')))"
            )
        } else {
            format!(
                "(NOT regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '\\bcreate\\b') \
                  OR (regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '{DUCKLAKE_INLINED_CREATE_RE}') \
                      AND NOT regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '\\bas\\s+(select|with|values|from|table)\\b')))"
            )
        };
        format!(
            "AND (strpos(p_sql, ';') = 0 \
                  OR regexp_matches(p_sql, '^begin(\\s+transaction)?\\s*;[\\s\\S]*;\\s*(commit|rollback|abort)$')) \
             AND list_bool_and(list_transform(string_split(p_sql, ';'), lambda _stmt: (\
                  regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g') = '' \
                  OR ((strpos(p_sql, ';') = 0 \
                       OR regexp_matches(regexp_replace(_stmt, '^\\s+|\\s+$', '', 'g'), '^(begin(\\s+transaction)?|commit|rollback|abort|select|with|values|insert|update|delete|merge|create)\\b')) \
                      AND {per_stmt_create_ok})\
             )))"
        )
    } else if config.writer_allow_temp_tables {
        format!(
            "AND strpos(p_sql, ';') = 0 \
             AND (NOT regexp_matches(p_sql, '\\bcreate\\b') \
                  OR regexp_matches(p_sql, '{TEMP_CREATE_RE}'))"
        )
    } else {
        "AND strpos(p_sql, ';') = 0 \
         AND NOT regexp_matches(p_sql, '\\bcreate\\b')"
            .to_string()
    };

    // In `ducklake_mode`, block `writer` from mutating DuckLake catalog/schema DDL metadata tables
    // (while permitting SELECT reads on them and DML on snapshot/data_file/delete_file/stats/inlined tables).
    // Matches both unquoted (`main.ducklake_table(`) and double-quoted (`"main" . "ducklake_table"(`) forms.
    const DUCKLAKE_DDL_TABLES: &str = "ducklake_schema|ducklake_schema_versions|ducklake_table|ducklake_column|ducklake_column_mapping|ducklake_name_mapping|ducklake_view|ducklake_metadata|ducklake_partition_info|ducklake_partition_column|ducklake_sort_info|ducklake_sort_expression|ducklake_macro|ducklake_macro_impl|ducklake_macro_parameters|ducklake_tag|ducklake_column_tag";
    let writer_ducklake_ddl_rule = if config.ducklake_mode {
        format!(
            "AND NOT regexp_matches(p_sql, '\\b(insert(\\s+or\\s+(replace|ignore))?\\s+into|update|delete\\s+from|merge\\s+into)\\s+([a-z0-9_\"]+\\s*\\.\\s*)*(\"({DUCKLAKE_DDL_TABLES})\"|({DUCKLAKE_DDL_TABLES})\\b)')"
        )
    } else {
        String::new()
    };

    let writer_copy_rule = if config.allow_copy {
        ""
    } else {
        "AND NOT regexp_matches(p_sql, '\\bcopy\\b')"
    };

    // Shared internal/sensitive/exfiltration symbols blocked for all non-admin roles.
    const SENSITIVE_IDENT_DENYLIST: &str = "\\b(_duckpack_auth|_duckpack_quack_authenticate|_duckpack_quack_authorize|quack_users|quack_sessions|quack_authentication_function|quack_authorization_function|quack_serve|quack_stop|quack_cancel|quack_query|quack_query_by_name|quack_generate_keys|quack_identify|quack_active_connections|quack_server_list|quack_check_token|quack_nop_authorization|enable_logging|disable_logging|duckdb_logs|duckdb_logs_parsed|duckdb_secrets|which_secret|getenv|current_setting|duckdb_settings|json_execute_serialized_sql|read_text|read_blob|glob)\\b";

    // Block SET and PRAGMA for reader (in DuckDB v2.0 bare SET defaults to GLOBAL scope, and PRAGMA can write files).
    let reader_prefix_pattern = if config.reader_allow_explain_describe {
        "^(select|with|values|from|table|explain|describe|desc|show|summarize|use|begin|start|commit|rollback|abort)\\b"
    } else {
        "^(select|with|values|from|table|use|begin|start|commit|rollback|abort)\\b"
    };

    sql.push_str(&format!(
        "CREATE OR REPLACE MACRO _duckpack_auth.authorize_sanitized(p_role, p_sql) AS (\n\
         \x20   CASE p_role\n\
         \x20       WHEN 'admin' THEN true\n\
         \x20       WHEN 'writer' THEN (\n\
         \x20           p_sql <> ''\n\
         \x20           AND regexp_matches(p_sql, '{writer_prefix_pattern}')\n\
         \x20           AND NOT regexp_matches(p_sql, '{SENSITIVE_IDENT_DENYLIST}')\n\
         \x20           AND NOT regexp_matches(p_sql, '\\b(attach|detach|install|load|export|import|drop|alter|truncate|vacuum|rename|grant|revoke|call|checkpoint|pragma|prepare|execute|deallocate)\\b')\n\
         \x20           AND NOT regexp_matches(p_sql, '\\b(update\\s+extensions|comment\\s+on)\\b')\n\
         \x20           AND NOT regexp_matches(p_sql, '^(set|reset)\\b')\n\
         \x20           AND NOT regexp_matches(p_sql, '\\b(set|reset)\\s+(global|session|local)\\b')\n\
         \x20           AND NOT regexp_matches(p_sql, '\\b(create|drop)\\s+(or\\s+replace\\s+)?(persistent\\s+|temporary\\s+|temp\\s+)?secret\\b')\n\
         \x20           {writer_batch_and_create_rule}\n\
         \x20           {writer_ducklake_ddl_rule}\n\
         \x20           {writer_copy_rule}\n\
         \x20       )\n\
         \x20       WHEN 'reader' THEN (\n\
         \x20           p_sql <> ''\n\
         \x20           AND strpos(p_sql, ';') = 0\n\
         \x20           AND regexp_matches(p_sql, '{reader_prefix_pattern}')\n\
         \x20           AND NOT regexp_matches(p_sql, '{SENSITIVE_IDENT_DENYLIST}')\n\
         \x20           AND NOT regexp_matches(p_sql, '\\b(insert|update|delete|merge|create|alter|drop|truncate|copy|call|vacuum|rename|grant|revoke|prepare|execute|deallocate|attach|detach|install|load|export|import|set|reset|pragma|checkpoint|comment)\\b')\n\
         \x20       )\n\
         \x20       ELSE false\n\
         \x20   END\n\
         );\n\
         CREATE OR REPLACE MACRO _duckpack_auth.authorize(p_sid, p_query) AS (\n\
         \x20   (\n\
         \x20       WITH _r AS MATERIALIZED (SELECT _duckpack_auth.resolve_role(p_sid) AS role),\n\
         \x20            _s AS MATERIALIZED (SELECT CASE WHEN (SELECT role FROM _r) = 'admin' THEN '' ELSE _duckpack_auth.sanitize_sql(p_query) END AS q)\n\
         \x20       SELECT _duckpack_auth.authorize_sanitized(_r.role, _s.q)\n\
         \x20       FROM _r, _s\n\
         \x20   )\n\
         );\n\
         CREATE OR REPLACE MACRO _duckpack_quack_authenticate(p_sid, p_client_token, p_server_token) AS (_duckpack_auth.authenticate(p_sid, p_client_token, p_server_token));\n\
         CREATE OR REPLACE MACRO _duckpack_quack_authorize(p_sid, p_query) AS (_duckpack_auth.authorize(p_sid, p_query));\n"
    ));

    if include_global_hooks {
        if config.enabled {
            sql.push_str(
                "SET GLOBAL quack_authentication_function = '_duckpack_quack_authenticate';\n\
                 SET GLOBAL quack_authorization_function = '_duckpack_quack_authorize';\n",
            );
        } else {
            sql.push_str(
                "SET GLOBAL quack_authentication_function = 'quack_check_token';\n\
                 SET GLOBAL quack_authorization_function = 'quack_nop_authorization';\n",
            );
        }
    }

    Ok(sql)
}

/// Full compiler for `_duckpack_auth` using a local `project_dir` (for `.env` fallback).
pub fn compile_quack_auth_sql_with_options(
    config: &QuackAuthConfig,
    project_dir: &Path,
    deploying_sid: Option<&str>,
    master_client_id_hash: Option<&str>,
    live_client_id_hashes: &std::collections::HashMap<String, String>,
    quack_server_mode: bool,
    include_global_hooks: bool,
) -> Result<String> {
    compile_quack_auth_sql_with_options_opt(
        config,
        Some(project_dir),
        deploying_sid,
        master_client_id_hash,
        live_client_id_hashes,
        quack_server_mode,
        include_global_hooks,
    )
}

/// Writes a standalone server startup bootstrap SQL script to `project_dir/scripts/quack_rbac_init.sql`.
pub fn export_quack_rbac_bootstrap_script(
    config: &QuackAuthConfig,
    project_dir: &Path,
) -> Result<PathBuf> {
    let sql = compile_quack_auth_sql(config, project_dir, None, true)?;
    let scripts_dir = project_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir)?;
    let out_path = scripts_dir.join("quack_rbac_init.sql");
    crate::engine::write_restricted_env_file(&out_path, &sql)?;
    Ok(out_path)
}

/// Queries a live Quack server for:
/// 1. The deploying connection's own `connection_id` (via `quack_connection_id('<qa>')`) and its
///    server-salted `client_id_hash` (used to bind `__duckpack_master` as `admin`).
/// 2. Each configured user's exact server-salted `client_id_hash` by probing the Quack server via
///    `quack_query(..., client_id = '<raw_client_id>')`.
fn harvest_quack_client_id_hashes_opt(
    target_exec: &mut dyn DbExecutor,
    qa: &str,
    config: &QuackAuthConfig,
    project_dir: Option<&Path>,
) -> (
    Option<String>,
    Option<String>,
    std::collections::HashMap<String, String>,
) {
    let mut live_hashes = std::collections::HashMap::new();
    let quoted_qa = format!("\"{}\"", qa.replace('"', "\"\""));
    let safe_qa_lit = qa.replace('\'', "''");

    // 1. Get our exact connection_id via quack_connection_id('<qa>')
    let deploying_sid = target_exec
        .query_rows(&format!(
            "SELECT CAST(quack_connection_id('{}') AS VARCHAR)",
            safe_qa_lit
        ))
        .ok()
        .and_then(|(_, rows)| rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .filter(|s| !s.is_empty() && s != "NULL")
        .or_else(|| {
            let fallback_q = format!(
                "SELECT connection_id FROM {}.query('SELECT CAST(connection_id AS VARCHAR) AS connection_id FROM quack_active_connections() ORDER BY query_started_at DESC NULLS LAST LIMIT 1')",
                quoted_qa
            );
            target_exec
                .query_rows(&fallback_q)
                .ok()
                .and_then(|(_, rows)| rows.into_iter().next())
                .and_then(|r| r.into_iter().next())
                .filter(|s| !s.is_empty() && s != "NULL")
        });

    // 2. Get the deploying master connection's live server-salted client_id_hash
    let master_client_id_hash = if let Some(ref sid) = deploying_sid {
        let safe_sid = sid.replace('\'', "''");
        let cid_q = format!(
            "SELECT client_id_hash FROM {}.query('SELECT CAST(client_id_hash AS VARCHAR) AS client_id_hash FROM quack_active_connections() WHERE lower(CAST(connection_id AS VARCHAR)) = lower(''{}'') LIMIT 1')",
            quoted_qa, safe_sid
        );
        target_exec
            .query_rows(&cid_q)
            .ok()
            .and_then(|(_, rows)| rows.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .filter(|s| !s.is_empty() && s != "NULL")
            .map(|s| s.to_lowercase())
    } else {
        None
    };

    // 3. Look up the attached Quack server's URI so we can harvest each user's live salted client_id_hash
    let quack_uri = target_exec
        .query_rows(&format!(
            "SELECT path FROM duckdb_databases() WHERE database_name = '{}' LIMIT 1",
            safe_qa_lit
        ))
        .ok()
        .and_then(|(_, rows)| rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .filter(|s| s.starts_with("quack:"));

    if let Some(uri) = quack_uri {
        let disable_ssl = crate::engine::should_disable_quack_ssl(&uri, false);
        let safe_uri = uri.replace('\'', "''");
        let ambient_master_token = crate::engine::resolve_quack_token(None, None);
        for (idx, u) in config.users.iter().enumerate() {
            let Some(user_token) = resolve_user_token_opt(project_dir, &u.token_env) else {
                continue;
            };
            let raw_cid = u.client_id.as_deref().unwrap_or(&user_token);
            if raw_cid.chars().count() < 4 {
                continue;
            }
            let safe_cid = raw_cid.replace('\'', "''");
            let nonce = format!(
                "__dp_probe_{}_{}",
                idx,
                std::process::id()
            );
            let inner_sql = format!(
                "SELECT CAST(client_id_hash AS VARCHAR) AS client_id_hash FROM quack_active_connections() WHERE query LIKE '%{}%' LIMIT 1",
                nonce
            );
            let safe_inner = inner_sql.replace('\'', "''");

            // Try probing with explicit token so inline-token attaches (Ctrl+A, Workspace, Ctrl+U)
            // without a matching session QUACK secret still harvest live server-salted hashes.
            // Order: ambient master token (works on initial bootstrap before RBAC is active),
            // user's own token (works post-RBAC deploy), then implicit session secret fallback.
            let mut candidate_tokens: Vec<Option<String>> = Vec::new();
            if let Some(ref mt) = ambient_master_token {
                candidate_tokens.push(Some(mt.clone()));
            }
            if ambient_master_token.as_deref() != Some(user_token.as_str()) {
                candidate_tokens.push(Some(user_token.clone()));
            }
            candidate_tokens.push(None);

            // Never silently downgrade TLS to plaintext on a TLS endpoint (H3).
            let ssl_modes: [bool; 1] = [disable_ssl];

            let mut probed: Option<String> = None;
            'probe: for &ssl_off in &ssl_modes {
                for tok_opt in &candidate_tokens {
                    let probe_sql = if let Some(tok) = tok_opt {
                        format!(
                            "SELECT client_id_hash FROM quack_query('{}', '{}', token = '{}', client_id = '{}', disable_ssl = {})",
                            safe_uri,
                            safe_inner,
                            tok.replace('\'', "''"),
                            safe_cid,
                            ssl_off
                        )
                    } else {
                        format!(
                            "SELECT client_id_hash FROM quack_query('{}', '{}', client_id = '{}', disable_ssl = {})",
                            safe_uri, safe_inner, safe_cid, ssl_off
                        )
                    };
                    if let Ok((_, rows)) = target_exec.query_rows(&probe_sql) {
                        if let Some(h) = rows
                            .into_iter()
                            .next()
                            .and_then(|r| r.into_iter().next())
                            .filter(|s| !s.is_empty() && s != "NULL")
                        {
                            probed = Some(h.to_lowercase());
                            break 'probe;
                        }
                    }
                }
            }

            if let Some(h) = probed {
                live_hashes.insert(u.username.clone(), h);
            }
        }
    }

    (deploying_sid, master_client_id_hash, live_hashes)
}

/// Introspects the target database's `_duckpack_auth.quack_users` table and compares it against
/// `duckpack.toml` (`[quack_auth]`). If any user, role, token hash, client_id_hash, macro, or
/// global hook has drifted, or if `_duckpack_auth` has not been deployed yet, appends a declarative
/// `DiffItem` to `diff_result`.
pub fn append_quack_auth_diff_items(
    project_dir: &Path,
    target_exec: &mut dyn DbExecutor,
    _catalog: Option<&str>,
    diff_result: &mut DiffResult,
) {
    let Some(config) = load_quack_auth_config(project_dir) else {
        return;
    };
    if !config.enabled {
        return;
    }
    let Ok(expected_users) = validate_quack_auth_config(&config, project_dir) else {
        return;
    };
    let q_alias = crate::diff::find_any_quack_alias(target_exec, _catalog);
    let is_quack_target = q_alias.is_some();

    // Query existing _duckpack_auth.quack_users FIRST so we can detect non-admin (Authorization failed)
    // callers immediately without probing or emitting misleading RBAC diffs.
    let query = "SELECT username, role, token_sha256, lower(client_id_hash), CAST(enabled AS VARCHAR) FROM _duckpack_auth.quack_users WHERE username <> '__duckpack_master' ORDER BY username";
    let ducklake_macro_check = if config.ducklake_mode {
        "AND COALESCE(macro_definition, '') LIKE '%ducklake_schema%'"
    } else {
        "AND COALESCE(macro_definition, '') NOT LIKE '%ducklake_schema%'"
    };
    let macros_query = if is_quack_target {
        format!(
            "SELECT count(DISTINCT function_name) FROM duckdb_functions() WHERE schema_name = '_duckpack_auth' AND function_name IN ('authenticate', 'authorize', 'authorize_sanitized', 'resolve_role', 'sanitize_sql') AND (function_name <> 'authenticate' OR COALESCE(macro_definition, '') LIKE '%quack_check_token%') AND (function_name <> 'authorize_sanitized' OR (COALESCE(macro_definition, '') LIKE '%duckdb_secrets%' {ducklake_macro_check})) AND (function_name <> 'resolve_role' OR (COALESCE(macro_definition, '') LIKE '%ac.connection_id%' AND COALESCE(macro_definition, '') LIKE '%WHEN ''none'' THEN 1%')) AND (function_name <> 'sanitize_sql' OR COALESCE(macro_definition, '') LIKE '%_lex%')"
        )
    } else {
        format!(
            "SELECT count(DISTINCT function_name) FROM duckdb_functions() WHERE schema_name = '_duckpack_auth' AND function_name IN ('authenticate', 'authorize', 'authorize_sanitized', 'resolve_role', 'sanitize_sql') AND (function_name <> 'authorize_sanitized' OR (COALESCE(macro_definition, '') LIKE '%duckdb_secrets%' {ducklake_macro_check})) AND (function_name <> 'sanitize_sql' OR COALESCE(macro_definition, '') LIKE '%_lex%')"
        )
    };
    let wrapper_query = "SELECT count(DISTINCT function_name) FROM duckdb_functions() WHERE schema_name = 'main' AND function_name IN ('_duckpack_quack_authenticate', '_duckpack_quack_authorize')";
    let hooks_query = "SELECT count(*) FROM duckdb_settings() WHERE (name = 'quack_authentication_function' AND value = '_duckpack_quack_authenticate') OR (name = 'quack_authorization_function' AND value = '_duckpack_quack_authorize')";

    let (actual_rows, macros_count, wrappers_count, hooks_active) = if let Some(ref qa) = q_alias {
        let quoted_qa = format!("\"{}\"", qa.replace('"', "\"\""));
        let remote_q = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            query.replace('\'', "''")
        );
        let rows_res = target_exec
            .query_rows(&remote_q)
            .or_else(|e| {
                if e.to_string().contains("Authorization failed") {
                    Err(e)
                } else {
                    target_exec.query_rows(query)
                }
            });
        if let Err(ref e) = rows_res {
            if e.to_string().contains("Authorization failed") {
                // Non-admin caller (reader/writer) cannot read or manage _duckpack_auth
                return;
            }
        }
        let rows = rows_res.ok().map(|(_, rows)| rows);

        let remote_m = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            macros_query.replace('\'', "''")
        );
        let m_count = target_exec
            .query_rows(&remote_m)
            .or_else(|_| target_exec.query_rows(&macros_query))
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);

        let remote_w = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            wrapper_query.replace('\'', "''")
        );
        let w_count = target_exec
            .query_rows(&remote_w)
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);

        let remote_h = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            hooks_query.replace('\'', "''")
        );
        let h_count = target_exec
            .query_rows(&remote_h)
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);

        (rows, m_count, w_count, h_count)
    } else {
        let rows = target_exec.query_rows(query).ok().map(|(_, rows)| rows);
        let m_count = target_exec
            .query_rows(&macros_query)
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let w_count = target_exec
            .query_rows(wrapper_query)
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        (rows, m_count, w_count, 2)
    };

    // Harvest the live Quack server's salted client_id_hash values for deployer & every user
    let (deploying_sid, master_cid_hash, live_hashes) = if let Some(ref qa) = q_alias {
        harvest_quack_client_id_hashes_opt(target_exec, qa, &config, Some(project_dir))
    } else {
        (None, None, std::collections::HashMap::new())
    };

    // Compile WITHOUT SET GLOBAL hooks inside the DDL batch (apply_diff activates hooks AFTER COMMIT!)
    let Ok(compiled_sql) = compile_quack_auth_sql_with_options(
        &config,
        project_dir,
        deploying_sid.as_deref(),
        master_cid_hash.as_deref(),
        &live_hashes,
        is_quack_target,
        false,
    ) else {
        return;
    };

    let mut sorted_expected = expected_users.clone();
    sorted_expected.sort_by(|a, b| a.0.username.cmp(&b.0.username));

    let is_in_sync = if macros_count < 5 || wrappers_count < 2 || hooks_active < 2 {
        false
    } else if let Some(ref rows) = actual_rows {
        if rows.len() != sorted_expected.len() {
            false
        } else {
            rows.iter().zip(sorted_expected.iter()).all(|(r, (u, th, ch))| {
                let expected_cid_hash = live_hashes
                    .get(&u.username)
                    .map(|s| s.as_str())
                    .unwrap_or(ch.as_str());
                r.len() >= 5
                    && r[0] == u.username
                    && r[1] == u.role.as_str()
                    && r[2] == *th
                    && r[3].eq_ignore_ascii_case(expected_cid_hash)
                    && (r[4].eq_ignore_ascii_case("true") || r[4] == "1") == u.enabled
            })
        }
    } else {
        false
    };

    if !is_in_sync {
        let old_summary = actual_rows.map(|rows| {
            rows.into_iter()
                .map(|r| {
                    if r.len() >= 5 {
                        format!(
                            "user={} role={} token_sha256={}.. enabled={}",
                            r[0],
                            r[1],
                            &r[2][..r[2].len().min(12)],
                            r[4]
                        )
                    } else {
                        r.join(",")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        let new_summary = sorted_expected
            .iter()
            .map(|(u, th, _)| {
                format!(
                    "user={} role={} token_sha256={}.. enabled={}",
                    u.username,
                    u.role.as_str(),
                    &th[..th.len().min(12)],
                    u.enabled
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        diff_result.items.push(DiffItem {
            item_type: DiffItemType::CreateMacro,
            obj_name: "_duckpack_auth.quack_rbac_policy".to_string(),
            sql: compiled_sql,
            old_def: old_summary,
            new_def: Some(new_summary),
            selected: true,
            ignored: false,
            locked: false,
        });
    }
}

/// Represents a live or recently active client session returned by `quack_active_connections()`,
/// enriched with resolved RBAC identity (`username`, `role`, `revoked`) from `_duckpack_auth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveQuackSession {
    pub server_id: String,
    pub connection_id: String,
    pub client_id_hash: String,
    pub username: String,
    pub role: String,
    pub revoked: bool,
    pub state: String,
    pub query: String,
    pub cached_rows: String,
    pub query_started_at: String,
}

/// Loads all live sessions from `quack_active_connections()` on the target Quack server alias,
/// correlating each session with `_duckpack_auth.quack_sessions` and `_duckpack_auth.quack_users`.
/// Gracefully falls back to plain `quack_active_connections()` if `_duckpack_auth` has not been
/// deployed yet or if the caller is a non-admin.
pub fn load_live_quack_sessions(
    conn: &duckdb::Connection,
    quack_alias: &str,
    config: Option<&QuackAuthConfig>,
) -> Result<Vec<LiveQuackSession>> {
    let mut exec: &duckdb::Connection = conn;
    let quoted_qa = format!("\"{}\"", quack_alias.replace('"', "\"\""));
    let default_role = config
        .map(|c| c.default_role.as_str())
        .unwrap_or("reader");
    let safe_default = default_role.replace('\'', "''");

    let enriched_inner = format!(
        "SELECT \
            COALESCE(CAST(ac.server_id AS VARCHAR), '') AS server_id, \
            COALESCE(CAST(ac.connection_id AS VARCHAR), '') AS connection_id, \
            COALESCE(CAST(ac.client_id_hash AS VARCHAR), '') AS client_id_hash, \
            COALESCE( \
                (SELECT s.username FROM _duckpack_auth.quack_sessions s WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(ac.connection_id AS VARCHAR)) LIMIT 1), \
                (SELECT u.username FROM _duckpack_auth.quack_users u WHERE u.enabled = true AND lower(u.client_id_hash) = lower(CAST(ac.client_id_hash AS VARCHAR)) ORDER BY CASE u.role WHEN 'none' THEN 1 WHEN 'reader' THEN 2 WHEN 'writer' THEN 3 WHEN 'admin' THEN 4 ELSE 1 END LIMIT 1), \
                '<default>' \
            ) AS username, \
            COALESCE( \
                (SELECT CASE WHEN s.revoked THEN 'none' ELSE s.role END FROM _duckpack_auth.quack_sessions s WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(ac.connection_id AS VARCHAR)) LIMIT 1), \
                (SELECT u.role FROM _duckpack_auth.quack_users u WHERE u.enabled = true AND lower(u.client_id_hash) = lower(CAST(ac.client_id_hash AS VARCHAR)) ORDER BY CASE u.role WHEN 'none' THEN 1 WHEN 'reader' THEN 2 WHEN 'writer' THEN 3 WHEN 'admin' THEN 4 ELSE 1 END LIMIT 1), \
                '{}' \
            ) AS role, \
            COALESCE( \
                (SELECT CAST(s.revoked AS VARCHAR) FROM _duckpack_auth.quack_sessions s WHERE lower(CAST(s.sid AS VARCHAR)) = lower(CAST(ac.connection_id AS VARCHAR)) LIMIT 1), \
                'false' \
            ) AS revoked, \
            COALESCE(CAST(ac.state AS VARCHAR), '') AS state, \
            COALESCE(CAST(ac.query AS VARCHAR), '') AS query, \
            COALESCE(CAST(ac.cached_rows AS VARCHAR), '') AS cached_rows, \
            COALESCE(CAST(ac.query_started_at AS VARCHAR), '') AS query_started_at \
         FROM quack_active_connections() ac \
         ORDER BY ac.query_started_at DESC NULLS LAST",
        safe_default
    );

    let enriched_sql = format!(
        "SELECT * FROM {}.query('{}')",
        quoted_qa,
        enriched_inner.replace('\'', "''")
    );

    let rows = match exec.query_rows(&enriched_sql) {
        Ok((_, rows)) => rows,
        Err(_) => {
            // Fallback when _duckpack_auth is not yet deployed or caller lacks access to _duckpack_auth
            let plain_inner = format!(
                "SELECT \
                    COALESCE(CAST(server_id AS VARCHAR), ''), \
                    COALESCE(CAST(connection_id AS VARCHAR), ''), \
                    COALESCE(CAST(client_id_hash AS VARCHAR), ''), \
                    '<unmanaged>', \
                    '{}', \
                    'false', \
                    COALESCE(CAST(state AS VARCHAR), ''), \
                    COALESCE(CAST(query AS VARCHAR), ''), \
                    COALESCE(CAST(cached_rows AS VARCHAR), ''), \
                    COALESCE(CAST(query_started_at AS VARCHAR), '') \
                 FROM quack_active_connections() \
                 ORDER BY query_started_at DESC NULLS LAST",
                safe_default
            );
            let plain_sql = format!(
                "SELECT * FROM {}.query('{}')",
                quoted_qa,
                plain_inner.replace('\'', "''")
            );
            exec.query_rows(&plain_sql)
                .with_context(|| {
                    format!(
                        "Failed to query quack_active_connections() on '{}'",
                        quack_alias
                    )
                })?
                .1
        }
    };

    let mut sessions = Vec::new();
    for r in rows {
        if r.len() >= 10 {
            sessions.push(LiveQuackSession {
                server_id: r[0].clone(),
                connection_id: r[1].clone(),
                client_id_hash: r[2].clone(),
                username: r[3].clone(),
                role: r[4].clone(),
                revoked: r[5].eq_ignore_ascii_case("true") || r[5] == "1",
                state: r[6].clone(),
                query: r[7].clone(),
                cached_rows: r[8].clone(),
                query_started_at: r[9].clone(),
            });
        }
    }
    Ok(sessions)
}

/// Snapshot of a user deployed inside `_duckpack_auth.quack_users` on a live Quack server
/// (used by `Ctrl+U` to inspect server RBAC even in workspaces managed via a central `rbac.duckpack`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveQuackUser {
    pub username: String,
    pub role: String,
    pub enabled: bool,
    pub has_token_hash: bool,
    pub has_client_id_hash: bool,
}

/// Queries deployed RBAC users from `_duckpack_auth.quack_users` on a live Quack server.
/// Returns `Ok(Vec::new())` cleanly if `_duckpack_auth` is not yet deployed or caller is non-admin.
pub fn load_live_quack_users(
    conn: &duckdb::Connection,
    quack_alias: &str,
) -> Result<Vec<LiveQuackUser>> {
    let mut exec: &duckdb::Connection = conn;
    let inner = "SELECT \
        COALESCE(CAST(username AS VARCHAR), ''), \
        COALESCE(CAST(role AS VARCHAR), 'none'), \
        COALESCE(CAST(enabled AS VARCHAR), 'false'), \
        CASE WHEN COALESCE(token_sha256, '') <> '' THEN 'true' ELSE 'false' END, \
        CASE WHEN COALESCE(client_id_hash, '') <> '' THEN 'true' ELSE 'false' END \
        FROM _duckpack_auth.quack_users ORDER BY username";
    let sql = crate::sql_utils::wrap_quack_alias_query(quack_alias, inner);
    let rows = match exec.query_rows(&sql) {
        Ok((_, rows)) => rows,
        Err(_) => return Ok(Vec::new()),
    };
    let mut users = Vec::new();
    for r in rows {
        if r.len() >= 5 {
            users.push(LiveQuackUser {
                username: r[0].clone(),
                role: r[1].clone(),
                enabled: r[2].eq_ignore_ascii_case("true") || r[2] == "1",
                has_token_hash: r[3].eq_ignore_ascii_case("true") || r[3] == "1",
                has_client_id_hash: r[4].eq_ignore_ascii_case("true") || r[4] == "1",
            });
        }
    }
    Ok(users)
}

/// Cancels the currently running query for a live Quack session via the DuckDB v2.0
/// client-side table function `quack_cancel(catalog, connection_id)`.
pub fn cancel_quack_session_query(
    conn: &duckdb::Connection,
    quack_alias: &str,
    _server_id: &str,
    sid: &str,
) -> Result<()> {
    let safe_alias = quack_alias.replace('\'', "''");
    let safe_sid = sid.replace('\'', "''");
    let sql = format!("SELECT * FROM quack_cancel('{}', '{}')", safe_alias, safe_sid);
    let mut exec: &duckdb::Connection = conn;
    let (_, rows) = exec.query_rows(&sql).with_context(|| {
        format!(
            "Failed to cancel query for session '{}' on '{}'",
            sid, quack_alias
        )
    })?;
    if let Some(first_row) = rows.first() {
        if let Some(success_str) = first_row.get(1) {
            if success_str.eq_ignore_ascii_case("false") || success_str == "0" {
                anyhow::bail!(
                    "Quack server reported no active cancellable query for session '{}' on '{}'",
                    sid,
                    quack_alias
                );
            }
        }
    }
    Ok(())
}

/// Persists an explicit role override or revocation (`revoked = true`) for a live session ID
/// in `_duckpack_auth.quack_sessions` on the target Quack server.
pub fn set_quack_session_override(
    conn: &duckdb::Connection,
    quack_alias: &str,
    sid: &str,
    username: &str,
    role: &str,
    revoked: bool,
) -> Result<()> {
    let quoted_qa = format!("\"{}\"", quack_alias.replace('"', "\"\""));
    let safe_sid = sid.replace('\'', "''");
    let safe_user = username.replace('\'', "''");
    let safe_role = role.replace('\'', "''");
    let inner = format!(
        "CREATE SCHEMA IF NOT EXISTS _duckpack_auth; \
         CREATE TABLE IF NOT EXISTS _duckpack_auth.quack_sessions (sid VARCHAR PRIMARY KEY, username VARCHAR, role VARCHAR NOT NULL, revoked BOOLEAN NOT NULL DEFAULT false, updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP); \
         DELETE FROM _duckpack_auth.quack_sessions WHERE lower(CAST(sid AS VARCHAR)) = lower('{}'); \
         INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) VALUES ('{}', '{}', '{}', {});",
        safe_sid, safe_sid, safe_user, safe_role, revoked
    );
    let sql = format!(
        "CALL {}.query('{}')",
        quoted_qa,
        inner.replace('\'', "''")
    );
    conn.execute_batch(&sql).with_context(|| {
        format!(
            "Failed to update session override for '{}' on '{}'",
            sid, quack_alias
        )
    })?;
    Ok(())
}

/// Deploys or syncs ONLY the declarative Quack RBAC policy from an already-loaded `QuackAuthConfig`
/// (`_duckpack_auth` tables, harvested live salted `client_id_hash` values, macros, and post-COMMIT
/// `SET GLOBAL` hooks) without touching any user tables, views, or pending schema migrations.
/// When `project_dir` is `None` (portable `.duckpack` bundle mode), tokens are resolved purely from OS env vars.
pub fn deploy_quack_rbac_from_config(
    exec: &mut dyn DbExecutor,
    quack_alias: Option<&str>,
    config: &QuackAuthConfig,
    project_dir: Option<&Path>,
) -> Result<usize> {
    let resolved_users = validate_quack_auth_config_opt(config, project_dir)?;
    let qa_opt = crate::diff::find_any_quack_alias(exec, quack_alias);
    let is_quack = qa_opt.is_some();

    let (deploying_sid, master_cid_hash, live_hashes) = if let Some(ref qa) = qa_opt {
        harvest_quack_client_id_hashes_opt(exec, qa, config, project_dir)
    } else {
        (None, None, std::collections::HashMap::new())
    };

    let compiled_sql = compile_quack_auth_sql_with_options_opt(
        config,
        project_dir,
        deploying_sid.as_deref(),
        master_cid_hash.as_deref(),
        &live_hashes,
        is_quack,
        false,
    )?;

    if let Some(ref qa) = qa_opt {
        let quoted_qa = format!("\"{}\"", qa.replace('"', "\"\""));
        let safe_sql = compiled_sql.replace('\'', "''");
        exec.execute_batch(&format!("CALL {}.query('BEGIN TRANSACTION;');", quoted_qa))?;
        let deploy_res = exec.execute_batch(&format!("CALL {}.query('{}')", quoted_qa, safe_sql));
        if let Err(e) = deploy_res {
            let _ = exec.execute_batch(&format!("CALL {}.query('ROLLBACK;');", quoted_qa));
            return Err(e).with_context(|| {
                format!("Failed deploying _duckpack_auth RBAC policy to '{}'", qa)
            });
        }
        exec.execute_batch(&format!("CALL {}.query('COMMIT;');", quoted_qa))
            .with_context(|| format!("Failed to COMMIT _duckpack_auth policy on '{}'", qa))?;

        // Activate (or disable) global hooks strictly AFTER COMMIT succeeds
        let (auth_fn, authz_fn) = if config.enabled {
            ("_duckpack_quack_authenticate", "_duckpack_quack_authorize")
        } else {
            ("quack_check_token", "quack_nop_authorization")
        };
        exec.execute_batch(&format!(
            "CALL {}.query('SET GLOBAL quack_authentication_function = ''{}'';');",
            quoted_qa, auth_fn
        ))
        .with_context(|| {
            format!(
                "Failed to set quack_authentication_function='{}' on '{}'",
                auth_fn, qa
            )
        })?;
        exec.execute_batch(&format!(
            "CALL {}.query('SET GLOBAL quack_authorization_function = ''{}'';');",
            quoted_qa, authz_fn
        ))
        .with_context(|| {
            format!(
                "Failed to set quack_authorization_function='{}' on '{}'",
                authz_fn, qa
            )
        })?;
    } else {
        exec.execute_batch(&compiled_sql)
            .with_context(|| "Failed deploying _duckpack_auth RBAC policy locally")?;
    }

    Ok(resolved_users.len())
}

/// Deploys or syncs ONLY the declarative Quack RBAC policy from `project_dir/duckpack.toml`.
/// Used by both `duckpack apply --rbac-only` and the `Ctrl+U` TUI Deploy action.
pub fn deploy_quack_rbac_now(
    exec: &mut dyn DbExecutor,
    quack_alias: Option<&str>,
    project_dir: &Path,
) -> Result<usize> {
    let config = load_quack_auth_config(project_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "No [quack_auth] section found in {}/duckpack.toml.",
            project_dir.display()
        )
    })?;
    deploy_quack_rbac_from_config(exec, quack_alias, &config, Some(project_dir))
}

/// Generates an executable, idempotent post-start shell script (`scripts/quack_rbac_post_start.sh`)
/// ready for Docker `ENTRYPOINT`, Compose, Kubernetes `postStart`, or `systemd ExecStartPost`.
/// Waits for the Quack server to accept connections and runs `duckpack apply --env <profile> --rbac-only --auto-approve`
/// to re-harvest the restarted server's salted `client_id_hash` values and re-activate the global RBAC hooks
/// without touching any user tables or pending schema migrations.
pub fn export_quack_rbac_post_start_script(
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Result<PathBuf> {
    let scripts_dir = project_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir)?;
    let out_path = scripts_dir.join("quack_rbac_post_start.sh");
    let default_env = env_profile
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("prod");
    crate::engine::validate_safe_env_profile(default_env)?;

    let script = format!(
        r#"#!/usr/bin/env bash
# DuckPack Quack RBAC Post-Start Hook
#
# Why this script exists:
#   Even though _duckpack_auth tables & macros persist inside your .duckdb file on disk,
#   DuckDB/Quack resets two in-memory server states on every container/service restart:
#     1. SET GLOBAL quack_authentication_function / quack_authorization_function
#     2. Quack's per-server random salt used for quack_active_connections().client_id_hash
#
#   This script waits for the Quack server to become reachable and runs:
#     duckpack apply --env "$DUCKPACK_ENV" --rbac-only --auto-approve
#   which ONLY syncs _duckpack_auth (re-harvesting live salted hashes + re-activating hooks)
#   and NEVER modifies or drops user tables or runs pending schema migrations.
#
# Usage examples:
#   1. Standalone / CI-CD:
#        ./scripts/quack_rbac_post_start.sh
#        DUCKPACK_ENV=staging ./scripts/quack_rbac_post_start.sh
#   2. Docker entrypoint (background post-start alongside quack_serve):
#        ./scripts/quack_rbac_post_start.sh &
#        exec duckdb /data/warehouse.duckdb -init <(echo "LOAD quack; CALL quack_serve('quack:0.0.0.0:9494', allow_other_hostname = true, token = '${{QUACK_MASTER_TOKEN}}', disable_ssl = true);") -no-stdin
#   3. Kubernetes lifecycle.postStart / systemd ExecStartPost:
#        ExecStartPost=/path/to/project/scripts/quack_rbac_post_start.sh

set -euo pipefail

PROJECT_DIR="${{DUCKPACK_PROJECT_DIR:-$(cd "$(dirname "${{BASH_SOURCE[0]}}")/.." && pwd)}}"
DUCKPACK_ENV="${{DUCKPACK_ENV:-{default_env}}}"
DUCKPACK_BIN="${{DUCKPACK_BIN:-duckpack}}"
RBAC_BUNDLE="${{DUCKPACK_RBAC_BUNDLE:-}}"
QUACK_DB="${{DUCKPACK_QUACK_DB:-quack:127.0.0.1:9494}}"
MAX_RETRIES="${{QUACK_RBAC_MAX_RETRIES:-30}}"
SLEEP_SECS="${{QUACK_RBAC_RETRY_SLEEP:-1}}"

if [ -n "${{RBAC_BUNDLE}}" ]; then
    echo "[duckpack-rbac] Waiting for Quack server (bundle=${{RBAC_BUNDLE}}, db=${{QUACK_DB}})..."
else
    echo "[duckpack-rbac] Waiting for Quack server (env=${{DUCKPACK_ENV}}, project=${{PROJECT_DIR}})..."
fi

for attempt in $(seq 1 "${{MAX_RETRIES}}"); do
    if [ -n "${{RBAC_BUNDLE}}" ]; then
        if "${{DUCKPACK_BIN}}" apply --from "${{RBAC_BUNDLE}}" --rbac-only --db "${{QUACK_DB}}" --auto-approve; then
            echo "[duckpack-rbac] Quack RBAC synced from bundle and global hooks activated (attempt ${{attempt}}/${{MAX_RETRIES}})."
            exit 0
        fi
    else
        if "${{DUCKPACK_BIN}}" apply --project-dir "${{PROJECT_DIR}}" --env "${{DUCKPACK_ENV}}" --rbac-only --auto-approve; then
            echo "[duckpack-rbac] Quack RBAC synced and global hooks activated (attempt ${{attempt}}/${{MAX_RETRIES}})."
            exit 0
        fi
    fi
    if [ "${{attempt}}" -lt "${{MAX_RETRIES}}" ]; then
        sleep "${{SLEEP_SECS}}"
    fi
done

echo "[duckpack-rbac] ERROR: Failed to sync Quack RBAC after ${{MAX_RETRIES}} attempts." >&2
exit 1
"#
    );

    std::fs::write(&out_path, script)
        .with_context(|| format!("Failed to write {}", out_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::Connection;

    #[test]
    fn test_sha256_hex_matches_duckdb_sha256() {
        let conn = Connection::open_in_memory().unwrap();
        for sample in ["", "super_secret_token_123", "alice_admin_tok", "bob_writer_!@#"] {
            let rust_hash = sha256_hex(sample);
            let db_hash: String = conn
                .query_row("SELECT sha256(?);", [sample], |r| r.get(0))
                .unwrap();
            assert_eq!(rust_hash, db_hash, "SHA-256 mismatch for input '{}'", sample);
        }
    }

    #[test]
    fn test_quack_rbac_compile_and_role_enforcement_in_duckdb() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_quack_rbac_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        save_env_key_value(&temp_dir, "QUACK_TOKEN_ALICE", "alice_secret_tok").unwrap();
        save_env_key_value(&temp_dir, "QUACK_TOKEN_BOB", "bob_secret_tok").unwrap();
        save_env_key_value(&temp_dir, "QUACK_TOKEN_CAROL", "carol_secret_tok").unwrap();

        let config = QuackAuthConfig {
            enabled: true,
            ducklake_mode: false,
            default_role: QuackRole::Reader,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
            allow_insecure_client_id: false,
            users: vec![
                QuackAuthUser {
                    username: "alice".to_string(),
                    role: QuackRole::Admin,
                    token_env: "QUACK_TOKEN_ALICE".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "bob".to_string(),
                    role: QuackRole::Writer,
                    token_env: "QUACK_TOKEN_BOB".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "carol".to_string(),
                    role: QuackRole::Reader,
                    token_env: "QUACK_TOKEN_CAROL".to_string(),
                    client_id: None,
                    enabled: true,
                },
            ],
        };

        save_quack_auth_config(&temp_dir, &config).unwrap();
        let loaded = load_quack_auth_config(&temp_dir).expect("Should load [quack_auth]");
        assert_eq!(loaded, config);

        let sql = compile_quack_auth_sql(&config, &temp_dir, Some("sid_deployer"), false).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&sql).unwrap();

        // Register active session IDs for bob (writer) and carol (reader)
        conn.execute_batch(
            "INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) VALUES
                ('sid_alice', 'alice', 'admin', false),
                ('sid_bob', 'bob', 'writer', false),
                ('sid_carol', 'carol', 'reader', false);",
        )
        .unwrap();

        // 1. Authentication checks
        let auth_alice: bool = conn
            .query_row(
                "SELECT _duckpack_auth.authenticate('sid_alice', 'alice_secret_tok', '');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(auth_alice);

        let auth_bad: bool = conn
            .query_row(
                "SELECT _duckpack_auth.authenticate('sid_alice', 'wrong_token', '');",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!auth_bad);

        // 2. Authorization checks helper
        let check_auth = |sid: &str, q: &str| -> bool {
            conn.query_row(
                "SELECT _duckpack_auth.authorize(?, ?);",
                [sid, q],
                |r| r.get(0),
            )
            .unwrap()
        };

        // Admin (alice & deployer) can run DDL, DML, and inspect _duckpack_auth
        assert!(check_auth("sid_alice", "DROP TABLE orders;"));
        assert!(check_auth("sid_deployer", "CREATE TABLE gold_metrics (id INT);"));
        assert!(check_auth("sid_alice", "SELECT * FROM _duckpack_auth.quack_users;"));

        // Writer (bob) can run SELECT, INSERT, UPDATE, DELETE, and CREATE TEMP TABLE
        assert!(check_auth("sid_bob", "SELECT * FROM orders;"));
        assert!(check_auth("sid_bob", "INSERT INTO orders VALUES (1, 100);"));
        assert!(check_auth("sid_bob", "UPDATE orders SET amount = 200 WHERE id = 1;"));
        assert!(check_auth("sid_bob", "DELETE FROM orders WHERE id = 1;"));
        assert!(check_auth("sid_bob", "CREATE TEMP TABLE stg_orders (id INT);"));

        // Writer (bob) is BLOCKED from permanent DDL, multi-statement chaining, and _duckpack_auth tampering!
        assert!(!check_auth("sid_bob", "CREATE TABLE orders (id INT);"));
        assert!(!check_auth("sid_bob", "DROP TABLE orders;"));
        assert!(!check_auth("sid_bob", "/* comment bypass */ DROP TABLE orders;"));
        assert!(!check_auth("sid_bob", "ALTER TABLE orders ADD COLUMN x INT;"));
        assert!(!check_auth("sid_bob", "TRUNCATE TABLE orders;"));
        assert!(!check_auth("sid_bob", "SELECT 1; DROP TABLE orders;"));
        assert!(!check_auth("sid_bob", "UPDATE _duckpack_auth.quack_users SET role = 'admin';"));
        assert!(!check_auth("sid_bob", "SELECT * FROM _duckpack_auth.quack_users;"));
        assert!(!check_auth("sid_bob", "SET GLOBAL quack_authorization_function = 'quack_nop_authorization';"));

        // Reader (carol) can run SELECT, WITH, EXPLAIN, DESCRIBE, and queries with 'DROP' inside string literals
        assert!(check_auth("sid_carol", "SELECT * FROM orders WHERE note = 'DROP TABLE orders';"));
        assert!(check_auth("sid_carol", "WITH cte AS (SELECT 1) SELECT * FROM cte;"));
        assert!(check_auth("sid_carol", "EXPLAIN SELECT * FROM orders;"));
        assert!(check_auth("sid_carol", "DESCRIBE orders;"));

        // Reader (carol) is BLOCKED from DML, temp tables, DDL, and _duckpack_auth
        assert!(!check_auth("sid_carol", "INSERT INTO orders VALUES (1, 100);"));
        assert!(!check_auth("sid_carol", "UPDATE orders SET amount = 0;"));
        assert!(!check_auth("sid_carol", "DELETE FROM orders;"));
        assert!(!check_auth("sid_carol", "CREATE TEMP TABLE t (id INT);"));
        assert!(!check_auth("sid_carol", "SELECT * FROM _duckpack_auth.quack_users;"));

        // Revoking a session immediately blocks both authenticate and authorize
        conn.execute_batch("UPDATE _duckpack_auth.quack_sessions SET revoked = true WHERE sid = 'sid_bob';")
            .unwrap();
        assert!(!check_auth("sid_bob", "SELECT 1;"));

        // 3. Verify drift detection when someone modifies _duckpack_auth.quack_users on the server
        let mut exec: &Connection = &conn;
        let mut diff_res = DiffResult { items: Vec::new() };
        append_quack_auth_diff_items(&temp_dir, &mut exec, None, &mut diff_res);
        assert!(diff_res.items.is_empty(), "Should have 0 drift when quack_users matches duckpack.toml");

        // Tamper with bob's role on the server -> append_quack_auth_diff_items must detect drift!
        conn.execute_batch("UPDATE _duckpack_auth.quack_users SET role = 'admin' WHERE username = 'bob';")
            .unwrap();
        append_quack_auth_diff_items(&temp_dir, &mut exec, None, &mut diff_res);
        assert_eq!(diff_res.items.len(), 1, "Must detect tampered role in _duckpack_auth.quack_users");
        assert_eq!(diff_res.items[0].obj_name, "_duckpack_auth.quack_rbac_policy");

        // 4. Verify Admin Lockout Prevention Guard rejects a config with zero admins
        let no_admin_cfg = QuackAuthConfig {
            enabled: true,
            default_role: QuackRole::Reader,
            users: vec![QuackAuthUser {
                username: "bob".to_string(),
                role: QuackRole::Writer,
                token_env: "QUACK_TOKEN_BOB".to_string(),
                client_id: None,
                enabled: true,
            }],
            ..Default::default()
        };
        let lockout_err = validate_quack_auth_config(&no_admin_cfg, &temp_dir)
            .unwrap_err()
            .to_string();
        assert!(lockout_err.contains("Admin Lockout Prevention Guard"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_quack_rbac_post_start_script_and_local_deploy_rbac_only() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_quack_post_start_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Verify post-start script export content & executable permissions
        let sh_path = export_quack_rbac_post_start_script(&temp_dir, Some("staging")).unwrap();
        assert_eq!(sh_path, temp_dir.join("scripts").join("quack_rbac_post_start.sh"));
        let content = std::fs::read_to_string(&sh_path).unwrap();
        assert!(content.starts_with("#!/usr/bin/env bash"));
        assert!(content.contains("--rbac-only --auto-approve"));
        assert!(content.contains("DUCKPACK_ENV:-staging"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&sh_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "Post-start hook script should be executable (0755)");
        }

        // 2. Verify deploy_quack_rbac_now deploys ONLY _duckpack_auth without touching pending user tables
        save_env_key_value(&temp_dir, "QUACK_TEST_ADMIN", "admin_secret_1234").unwrap();
        let cfg = QuackAuthConfig {
            enabled: true,
            default_role: QuackRole::Reader,
            users: vec![QuackAuthUser {
                username: "alice".to_string(),
                role: QuackRole::Admin,
                token_env: "QUACK_TEST_ADMIN".to_string(),
                client_id: None,
                enabled: true,
            }],
            ..Default::default()
        };
        save_quack_auth_config(&temp_dir, &cfg).unwrap();
        // Put a pending user table in tables/ that must NOT be created by deploy_quack_rbac_now
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();
        std::fs::write(
            temp_dir.join("tables").join("untouched_table.sql"),
            "CREATE TABLE untouched_table (id INT);",
        )
        .unwrap();

        let conn = Connection::open_in_memory().unwrap();
        let mut exec: &Connection = &conn;
        let synced = deploy_quack_rbac_now(&mut exec, None, &temp_dir).unwrap();
        assert_eq!(synced, 1);

        let user_cnt: i64 = conn
            .query_row("SELECT count(*) FROM _duckpack_auth.quack_users WHERE username = 'alice'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_cnt, 1);

        let untouched_exists: i64 = conn
            .query_row("SELECT count(*) FROM duckdb_tables() WHERE table_name = 'untouched_table'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            untouched_exists, 0,
            "deploy_quack_rbac_now (--rbac-only) must NOT create pending user tables"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ducklake_mode_rbac_ddl_vs_dml_separation() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_ducklake_rbac_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        save_env_key_value(&temp_dir, "DL_ADMIN_TOK", "dl_admin_secret_111").unwrap();
        save_env_key_value(&temp_dir, "DL_WRITER_TOK", "dl_writer_secret_222").unwrap();
        save_env_key_value(&temp_dir, "DL_READER_TOK", "dl_reader_secret_333").unwrap();

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
                    username: "admin".to_string(),
                    role: QuackRole::Admin,
                    token_env: "DL_ADMIN_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "writer".to_string(),
                    role: QuackRole::Writer,
                    token_env: "DL_WRITER_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "reader".to_string(),
                    role: QuackRole::Reader,
                    token_env: "DL_READER_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
            ],
        };
        save_quack_auth_config(&temp_dir, &cfg).unwrap();
        let reloaded = load_quack_auth_config(&temp_dir).unwrap();
        assert!(reloaded.ducklake_mode);

        let sql = compile_quack_auth_sql(&cfg, &temp_dir, Some("sid_admin"), false).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&sql).unwrap();
        conn.execute_batch(
            "INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) VALUES
                ('sid_w', 'writer', 'writer', false),
                ('sid_r', 'reader', 'reader', false);",
        )
        .unwrap();

        let check = |sid: &str, q: &str| -> bool {
            conn.query_row("SELECT _duckpack_auth.authorize(?, ?);", [sid, q], |r| r.get(0))
                .unwrap()
        };

        // Admin: full DuckLake DDL & DML access
        assert!(check("sid_admin", "INSERT INTO ducklake_table VALUES (1, 'uuid', 2, NULL, 0, 'orders');"));
        assert!(check("sid_admin", "UPDATE ducklake_schema SET end_snapshot = 5 WHERE schema_id = 1;"));

        // Writer: ALLOWED DuckLake DML, snapshot commits, stats, data-inlining tables, and SELECT on DDL metadata
        assert!(check("sid_w", "SELECT table_id, table_name FROM ducklake_table WHERE end_snapshot IS NULL;"));
        assert!(check("sid_w", "INSERT INTO ducklake_snapshot VALUES (5, now(), 1, 2, 3);"));
        assert!(check("sid_w", "INSERT INTO ducklake_snapshot_changes VALUES (5, 'inserted_into_table:1', NULL, NULL, NULL);"));
        assert!(check("sid_w", "INSERT INTO ducklake_data_file VALUES (1, 1, 5, NULL, 'a.parquet', true, 'parquet', 10, 100, 10, 0);"));
        assert!(check("sid_w", "INSERT INTO ducklake_delete_file VALUES (1, 1, 5, NULL, 1, 'd.parquet', true, 'parquet', 1, 50, 10);"));
        assert!(check("sid_w", "UPDATE ducklake_table_stats SET record_count = 10 WHERE table_id = 1;"));
        assert!(check("sid_w", "INSERT INTO ducklake_file_column_stats VALUES (1, 1, 1, 10, 0, '1', '10', false);"));
        assert!(check("sid_w", "INSERT INTO ducklake_inlined_data_tables VALUES (1, 'ducklake_inlined_data_1_1', 1);"));
        assert!(check("sid_w", "CREATE TABLE IF NOT EXISTS main.ducklake_inlined_data_1_1 (row_id BIGINT, begin_snapshot BIGINT, end_snapshot BIGINT, id INT);"));
        assert!(check("sid_w", "INSERT INTO ducklake_inlined_data_1_1 VALUES (0, 5, NULL, 42);"));
        assert!(check("sid_w", "UPDATE ducklake_inlined_data_1_1 SET end_snapshot = 6 WHERE row_id = 0;"));
        // Exact multi-statement BEGIN TRANSACTION; ...; COMMIT; batches emitted by DuckLake v2.0
        assert!(check(
            "sid_w",
            "BEGIN TRANSACTION;\nINSERT INTO \"main\".ducklake_snapshot VALUES (4, NOW(), 1, 2, 3);UPDATE \"main\".ducklake_table_stats SET record_count=28 WHERE table_id=1;INSERT INTO \"main\".ducklake_inlined_data_1_1 VALUES (26, 4, NULL, 2, 'a;b');INSERT INTO \"main\".ducklake_snapshot_changes VALUES (4, 'inlined_insert:1', NULL, NULL, NULL);\nCOMMIT;"
        ));
        assert!(check(
            "sid_w",
            "BEGIN TRANSACTION;\nINSERT INTO \"main\".ducklake_snapshot VALUES (4, NOW(), 1, 2, 2);CREATE TABLE IF NOT EXISTS \"main\".ducklake_inlined_delete_1(file_id BIGINT, row_id BIGINT, begin_snapshot BIGINT);\nINSERT INTO \"main\".ducklake_inlined_delete_1 VALUES (1, 6, 4);INSERT INTO \"main\".ducklake_snapshot_changes VALUES (4, 'inlined_delete:1', NULL, NULL, NULL);\nCOMMIT;"
        ));
        assert!(check(
            "sid_w",
            "BEGIN TRANSACTION;\nINSERT INTO \"main\".ducklake_snapshot VALUES (4, NOW(), 1, 2, 2);\nWITH deleted_row_list(deleted_row_id) AS (VALUES (0)) UPDATE \"main\".ducklake_inlined_data_1_1 SET _ducklake_end_snapshot = 4 FROM deleted_row_list WHERE _ducklake_row_id = deleted_row_id;\nINSERT INTO \"main\".ducklake_snapshot_changes VALUES (4, 'inlined_delete:1', NULL, NULL, NULL);\nCOMMIT;"
        ));

        // Writer: BLOCKED from DuckLake DDL metadata tables (CREATE/ALTER/DROP table, schema, view, metadata, partition, sort, tag, macro)
        assert!(!check("sid_w", "INSERT INTO ducklake_table VALUES (2, 'uuid', 5, NULL, 0, 'evil');"));
        assert!(!check("sid_w", "INSERT INTO \"main\" . \"ducklake_table\"(table_id) VALUES (2);"));
        assert!(!check("sid_w", "UPDATE main.ducklake_table SET end_snapshot = 5 WHERE table_id = 1;"));
        assert!(!check("sid_w", "INSERT INTO \"ducklake_column\" VALUES (1, 5, NULL, 1, 0, 'x', 'int32', true);"));
        assert!(!check("sid_w", "INSERT INTO ducklake_schema VALUES (2, 'uuid', 5, NULL, 'bad_schema');"));
        assert!(!check("sid_w", "UPDATE ducklake_schema SET end_snapshot = 5 WHERE schema_id = 1;"));
        assert!(!check("sid_w", "INSERT INTO ducklake_view VALUES (1, 'uuid', 5, NULL, 0, 'v', 'duckdb', 'SELECT 1');"));
        assert!(!check("sid_w", "UPDATE ducklake_metadata SET value = 'hacked' WHERE key = 'data_path';"));
        assert!(!check("sid_w", "INSERT INTO ducklake_partition_info VALUES (1, 1, 5, NULL);"));
        assert!(!check("sid_w", "INSERT INTO ducklake_sort_info VALUES (1, 1, 5, NULL);"));
        assert!(!check("sid_w", "INSERT INTO ducklake_tag VALUES (1, 5, NULL, 'k', 'v');"));
        assert!(!check("sid_w", "CREATE TABLE main.regular_user_table (id INT);"));
        // Writer: BLOCKED from DDL or statement smuggling inside BEGIN TRANSACTION; ...; COMMIT; batches
        assert!(!check(
            "sid_w",
            "BEGIN TRANSACTION; INSERT INTO \"main\".ducklake_snapshot VALUES (4, NOW(), 2, 3, 2); INSERT INTO \"main\".ducklake_table VALUES (2, 'uuid', 4, NULL, 0, 'evil', 'evil/', true); COMMIT;"
        ));
        assert!(!check(
            "sid_w",
            "BEGIN TRANSACTION; CREATE TABLE IF NOT EXISTS \"main\".ducklake_inlined_delete_1(id BIGINT); CREATE TABLE \"main\".evil_tbl(id INT); COMMIT;"
        ));
        assert!(!check(
            "sid_w",
            "BEGIN TRANSACTION; SELECT 1; PRAGMA enable_profiling; COMMIT;"
        ));
        assert!(!check("sid_w", "SELECT 1; SELECT 2;"));

        // Reader: strictly read-only
        assert!(check("sid_r", "SELECT * FROM ducklake_snapshot;"));
        assert!(check("sid_r", "SELECT * FROM ducklake_table;"));
        assert!(!check("sid_r", "INSERT INTO ducklake_snapshot VALUES (6, now(), 1, 2, 3);"));
        assert!(!check("sid_r", "INSERT INTO ducklake_data_file VALUES (2, 1, 6, NULL, 'b.parquet', true, 'parquet', 1, 10, 5, 0);"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_quack_rbac_adversarial_security_hardening() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_quack_sec_adv_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // L3: Unknown role string must fail closed to None (never Reader)
        assert_eq!(QuackRole::from_str("superadmin"), QuackRole::None);
        assert_eq!(QuackRole::from_str("typo_role"), QuackRole::None);

        // H2: Post-start script must reject shell injection / path traversal in env_profile
        assert!(export_quack_rbac_post_start_script(&temp_dir, Some("prod}; id; #")).is_err());
        assert!(export_quack_rbac_post_start_script(&temp_dir, Some("../../etc")).is_err());

        // M4: save_env_key_value must strip newline injection and enforce 0600 perms
        save_env_key_value(&temp_dir, "SEC_ADMIN_TOK", "admin_tok_111\nEVIL=1").unwrap();
        let env_raw = std::fs::read_to_string(temp_dir.join(".env")).unwrap();
        assert!(!env_raw.contains("\nEVIL=1"), "Newline injection must be stripped");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(temp_dir.join(".env")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        save_env_key_value(&temp_dir, "SEC_ADMIN_TOK", "admin_secret_1111").unwrap();
        save_env_key_value(&temp_dir, "SEC_WRITER_TOK", "writer_secret_2222").unwrap();
        save_env_key_value(&temp_dir, "SEC_READER_TOK", "reader_secret_3333").unwrap();
        save_env_key_value(&temp_dir, "SEC_DUP_TOK", "admin_secret_1111").unwrap();

        // C2: Reject plaintext client_id by default
        let insecure_cid_cfg = QuackAuthConfig {
            enabled: true,
            users: vec![QuackAuthUser {
                username: "alice".to_string(),
                role: QuackRole::Admin,
                token_env: "SEC_ADMIN_TOK".to_string(),
                client_id: Some("alice".to_string()),
                enabled: true,
            }],
            ..Default::default()
        };
        let err_cid = validate_quack_auth_config(&insecure_cid_cfg, &temp_dir).unwrap_err().to_string();
        assert!(err_cid.contains("Security Guard"), "Plaintext client_id must be rejected: {err_cid}");

        // C2: Reject duplicate secret token across distinct users
        let dup_tok_cfg = QuackAuthConfig {
            enabled: true,
            users: vec![
                QuackAuthUser {
                    username: "alice".to_string(),
                    role: QuackRole::Admin,
                    token_env: "SEC_ADMIN_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "bob".to_string(),
                    role: QuackRole::Reader,
                    token_env: "SEC_DUP_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
            ],
            ..Default::default()
        };
        let err_dup = validate_quack_auth_config(&dup_tok_cfg, &temp_dir).unwrap_err().to_string();
        assert!(err_dup.contains("duplicate secret token"), "Duplicate token must be rejected: {err_dup}");

        // Compile hardened policy (ducklake_mode = true to also test batch + inlined CREATE rules)
        let cfg = QuackAuthConfig {
            enabled: true,
            ducklake_mode: true,
            default_role: QuackRole::None,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
            allow_insecure_client_id: false,
            users: vec![
                QuackAuthUser {
                    username: "alice".to_string(),
                    role: QuackRole::Admin,
                    token_env: "SEC_ADMIN_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "bob".to_string(),
                    role: QuackRole::Writer,
                    token_env: "SEC_WRITER_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
                QuackAuthUser {
                    username: "carol".to_string(),
                    role: QuackRole::Reader,
                    token_env: "SEC_READER_TOK".to_string(),
                    client_id: None,
                    enabled: true,
                },
            ],
        };
        let sql = compile_quack_auth_sql(&cfg, &temp_dir, Some("sid_admin"), false).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&sql).unwrap();
        conn.execute_batch(
            "INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) VALUES
                ('sid_w', 'bob', 'writer', false),
                ('sid_r', 'carol', 'reader', false);",
        )
        .unwrap();

        let check = |sid: &str, q: &str| -> bool {
            conn.query_row("SELECT _duckpack_auth.authorize(?, ?);", [sid, q], |r| r.get(0))
                .unwrap()
        };

        // C1: Parser-differential bypass vectors MUST ALL BE BLOCKED for both reader & writer!
        let c1_payloads = [
            "SELECT '--'; DROP TABLE orders;",
            "SELECT '/*'; DROP TABLE orders; SELECT '*/'",
            "SELECT $a$'$a$; DROP TABLE orders; SELECT $b$'$b$",
            "SELECT 1 AS \"x--\"; DROP TABLE orders;",
            "SELECT 1 AS \"x/*\"; DROP TABLE orders; SELECT 1 AS \"*/\"",
            "SELECT e'abc\\''; DROP TABLE orders; SELECT ''",
            "SELECT e'abc\\\\'; DROP TABLE orders;",
            "/* outer /* inner */ still comment */ DROP TABLE orders;",
            "/* unclosed comment SELECT 1",
            "SELECT 'unclosed string",
            "SELECT $tag$unclosed dollar",
            "SELECT 1 AS \"unclosed_ident",
        ];
        for payload in c1_payloads {
            assert!(!check("sid_r", payload), "Reader MUST block C1 payload: {payload}");
            assert!(!check("sid_w", payload), "Writer MUST block C1 payload: {payload}");
        }

        // Legitimate tricky SQL with comments/nested comments/dollar quotes/escaped quotes MUST STILL PASS
        assert!(check("sid_r", "/* outer /* inner */ done */ SELECT 'DROP TABLE x', $tag$'; DROP TABLE y$tag$, e'it\\'s ok';"));
        assert!(check("sid_r", "SELECT 1 AS \"my-col\" FROM orders WHERE note = '-- not a comment';"));

        // C3: Bare SET, PRAGMA, getenv, duckdb_secrets, oracle macros, fs readers, CHECKPOINT, COMMENT, UPDATE EXTENSIONS
        let c3_blocked_both = [
            "SET memory_limit = '1MB';",
            "SET lock_configuration = true;",
            "RESET threads;",
            "PRAGMA enable_profiling = 'json';",
            "PRAGMA profiling_output = '/tmp/pwn';",
            "SELECT getenv('QUACK_MASTER_TOKEN');",
            "SELECT * FROM duckdb_secrets();",
            "SELECT which_secret('s3://bucket', 's3');",
            "SELECT current_setting('memory_limit');",
            "SELECT * FROM duckdb_settings();",
            "SELECT * FROM duckdb_logs();",
            "SELECT * FROM quack_active_connections();",
            "SELECT _duckpack_quack_authenticate('x', 'guess', '');",
            "SELECT _duckpack_quack_authorize('alice', 'DROP TABLE x');",
            "SELECT * FROM read_text('/etc/passwd');",
            "SELECT * FROM read_blob('/etc/passwd');",
            "SELECT * FROM glob('/etc/*');",
            "CHECKPOINT;",
            "FORCE CHECKPOINT;",
            "COMMENT ON TABLE orders IS 'pwned';",
            "PREPARE q AS SELECT 1;",
            "EXECUTE q;",
            "UPDATE EXTENSIONS;",
        ];
        for payload in c3_blocked_both {
            assert!(!check("sid_r", payload), "Reader MUST block C3 payload: {payload}");
            assert!(!check("sid_w", payload), "Writer MUST block C3 payload: {payload}");
        }

        // C3: DuckLake-mode hyphenated catalog DDL & CTAS inlined table abuse MUST BE BLOCKED
        assert!(!check("sid_w", "INSERT INTO \"my-catalog\".\"main\".\"ducklake_table\" VALUES (99);"));
        assert!(!check("sid_w", "CREATE TABLE \"main\".ducklake_inlined_data_1_1 AS SELECT 1;"));
        assert!(!check("sid_w", "CREATE TABLE \"main\".ducklake_inlined_data_1_1(id INT) AS SELECT 1;"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
