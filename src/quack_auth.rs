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
            _ => QuackRole::Reader,
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
    pub default_role: QuackRole,
    pub writer_allow_temp_tables: bool,
    pub reader_allow_explain_describe: bool,
    pub allow_copy: bool,
    pub users: Vec<QuackAuthUser>,
}

impl Default for QuackAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_role: QuackRole::Reader,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
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
        default_role,
        writer_allow_temp_tables,
        reader_allow_explain_describe,
        allow_copy,
        users,
    })
}

/// Resolves a user's cleartext token from process environment variables or `project_dir/.env`.
pub fn resolve_user_token(project_dir: &Path, token_env: &str) -> Option<String> {
    if let Ok(val) = std::env::var(token_env) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let env_path = project_dir.join(".env");
    if let Ok(content) = std::fs::read_to_string(env_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim() == token_env {
                    let clean_v = v
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .trim();
                    if !clean_v.is_empty() {
                        return Some(clean_v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Generates a 256-bit cryptographic random token and persists `<token_env>=<token>` in `project_dir/.env`
/// with `0600` permissions on Unix.
pub fn generate_and_save_user_token(project_dir: &Path, token_env: &str) -> Result<String> {
    let token = crate::engine::generate_random_hex_token();
    save_env_key_value(project_dir, token_env, &token)?;
    Ok(token)
}

/// Upserts `<key>=<value>` into `project_dir/.env` and enforces `0600` file permissions on Unix.
pub fn save_env_key_value(project_dir: &Path, key: &str, value: &str) -> Result<()> {
    let env_path = project_dir.join(".env");
    let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut found = false;
    let mut new_lines: Vec<String> = Vec::new();

    for line in existing.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    new_lines.push(format!("{}={}", key, value));
                    found = true;
                    continue;
                }
            }
        }
        new_lines.push(line.to_string());
    }
    if !found {
        new_lines.push(format!("{}={}", key, value));
    }

    let mut out = new_lines.join("\n");
    out.push('\n');
    std::fs::write(&env_path, out)
        .with_context(|| format!("Failed writing {} to {}", key, env_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600));
    }
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
pub fn validate_quack_auth_config(
    config: &QuackAuthConfig,
    project_dir: &Path,
) -> Result<Vec<(QuackAuthUser, String, String)>> {
    let mut resolved_users = Vec::new();
    let mut has_enabled_admin = config.default_role == QuackRole::Admin;

    for user in &config.users {
        validate_safe_username(&user.username)?;
        let token = resolve_user_token(project_dir, &user.token_env).ok_or_else(|| {
            anyhow::anyhow!(
                "Quack RBAC user '{}' references token_env '{}', which is not set in environment or .env.",
                user.username,
                user.token_env
            )
        })?;
        let token_hash = sha256_hex(&token);
        let raw_client_id = user.client_id.as_deref().unwrap_or(&token);
        let client_id_hash = sha256_hex(raw_client_id);

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
    let resolved_users = validate_quack_auth_config(config, project_dir)?;

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
    for (u, token_hash, client_id_hash) in &resolved_users {
        let safe_user = u.username.replace('\'', "''");
        let safe_role = u.role.as_str().replace('\'', "''");
        let safe_tok_hash = token_hash.replace('\'', "''");
        let safe_cid_hash = client_id_hash.replace('\'', "''");
        sql.push_str(&format!(
            "INSERT INTO _duckpack_auth.quack_users (username, role, token_sha256, client_id_hash, enabled) \
             VALUES ('{}', '{}', '{}', '{}', {});\n",
            safe_user, safe_role, safe_tok_hash, safe_cid_hash, u.enabled
        ));
    }

    // Auto-whitelist deploying session as 'admin' to prevent self-lockout
    if let Some(sid) = deploying_sid.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let safe_sid = sid.replace('\'', "''");
        sql.push_str(&format!(
            "DELETE FROM _duckpack_auth.quack_sessions WHERE sid = '{}';\n\
             INSERT INTO _duckpack_auth.quack_sessions (sid, username, role, revoked) \
             VALUES ('{}', '__duckpack_deployer', 'admin', false);\n",
            safe_sid, safe_sid
        ));
    }

    // 1. Authentication Macro: _duckpack_auth.authenticate(p_sid, p_client_token, p_server_token)
    sql.push_str(
        "CREATE OR REPLACE MACRO _duckpack_auth.authenticate(p_sid, p_client_token, p_server_token) AS (\n\
         \x20   CASE\n\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_sessions s\n\
         \x20           WHERE s.sid = p_sid AND s.revoked = true\n\
         \x20       ) THEN false\n\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_users u\n\
         \x20           WHERE u.enabled = true\n\
         \x20             AND u.token_sha256 = sha256(COALESCE(p_client_token, ''))\n\
         \x20       ) THEN true\n\
         \x20       ELSE false\n\
         \x20   END\n\
         );\n",
    );

    // 2. Role Resolution Macro: _duckpack_auth.resolve_role(p_sid)
    // Correlates p_sid via:
    //  1) Explicit session override / revocation in _duckpack_auth.quack_sessions
    //  2) Live Quack connection table quack_active_connections() (quack_connection_id = p_sid)
    //     matched against _duckpack_auth.quack_users.client_id_hash ("hashed client id")
    //  3) Direct username / client_id_hash / token_sha256 match on p_sid
    //  4) Configured default_role fallback
    let default_role_str = config.default_role.as_str();
    let quack_active_conn_clause = if include_global_hooks {
        "            (SELECT u.role FROM quack_active_connections() ac JOIN _duckpack_auth.quack_users u ON u.enabled = true AND (u.client_id_hash = CAST(ac.\"hashed client id\" AS VARCHAR) OR u.token_sha256 = CAST(ac.\"hashed client id\" AS VARCHAR)) WHERE CAST(ac.quack_connection_id AS VARCHAR) = CAST(p_sid AS VARCHAR) LIMIT 1),\n"
    } else {
        ""
    };
    sql.push_str(&format!(
        "CREATE OR REPLACE MACRO _duckpack_auth.resolve_role(p_sid) AS (\n\
         \x20   CASE\n\
         \x20       WHEN EXISTS (\n\
         \x20           SELECT 1 FROM _duckpack_auth.quack_sessions s\n\
         \x20           WHERE CAST(s.sid AS VARCHAR) = CAST(p_sid AS VARCHAR) AND s.revoked = true\n\
         \x20       ) THEN 'none'\n\
         \x20       ELSE COALESCE(\n\
         \x20           (SELECT s.role FROM _duckpack_auth.quack_sessions s WHERE CAST(s.sid AS VARCHAR) = CAST(p_sid AS VARCHAR) AND s.revoked = false LIMIT 1),\n\
         {quack_active_conn_clause}\
         \x20           (SELECT u.role FROM _duckpack_auth.quack_users u WHERE u.enabled = true AND (u.username = CAST(p_sid AS VARCHAR) OR u.client_id_hash = CAST(p_sid AS VARCHAR) OR u.token_sha256 = sha256(COALESCE(CAST(p_sid AS VARCHAR), ''))) LIMIT 1),\n\
         \x20           '{}'\n\
         \x20       )\n\
         \x20   END\n\
         );\n",
        default_role_str
    ));

    // 3. Query Sanitization Macro: _duckpack_auth.sanitize_sql(p_q)
    // Strips /* ... */ block comments, -- ... line comments, single-quoted string literals '...',
    // and trailing semicolons, then lowercases and trims.
    sql.push_str(
        "CREATE OR REPLACE MACRO _duckpack_auth.sanitize_sql(p_q) AS (\n\
         \x20   lower(trim(regexp_replace(\n\
         \x20       regexp_replace(\n\
         \x20           regexp_replace(\n\
         \x20               regexp_replace(COALESCE(p_q, ''), '/\\*[\\s\\S]*?\\*/', ' ', 'g'),\n\
         \x20               '--[^\\r\\n]*', ' ', 'g'\n\
         \x20           ),\n\
         \x20           '''([^'']|'''')*''', '''''', 'g'\n\
         \x20       ),\n\
         \x20       ';\\s*$', ''\n\
         \x20   )))\n\
         );\n",
    );

    // 4. Hardened Authorization Macro: _duckpack_auth.authorize(p_sid, p_query)
    let writer_create_rule = if config.writer_allow_temp_tables {
        "AND (NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\bcreate\\b') \
              OR regexp_matches(_duckpack_auth.sanitize_sql(p_query), '^create\\s+(or\\s+replace\\s+)?(temp|temporary)\\s+(table|view)\\b'))"
    } else {
        "AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\bcreate\\b')"
    };

    let writer_copy_rule = if config.allow_copy {
        ""
    } else {
        "AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\bcopy\\b')"
    };

    let reader_prefix_pattern = if config.reader_allow_explain_describe {
        "^(select|with|values|from|table|explain|describe|desc|show|summarize|use|pragma)\\b"
    } else {
        "^(select|with|values|from|table|use|pragma)\\b"
    };

    sql.push_str(&format!(
        "CREATE OR REPLACE MACRO _duckpack_auth.authorize(p_sid, p_query) AS (\n\
         \x20   CASE _duckpack_auth.resolve_role(p_sid)\n\
         \x20       WHEN 'admin' THEN true\n\
         \x20       WHEN 'writer' THEN (\n\
         \x20           _duckpack_auth.sanitize_sql(p_query) <> ''\n\
         \x20           AND strpos(_duckpack_auth.sanitize_sql(p_query), ';') = 0\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(_duckpack_auth|quack_users|quack_sessions|quack_authentication_function|quack_authorization_function|quack_serve|quack_stop|quack_cancel)\\b')\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(attach|detach|install|load|export|import|drop|alter|truncate|vacuum|rename|grant|revoke)\\b')\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(set|reset)\\s+(global|session)\\b')\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(create|drop)\\s+(or\\s+replace\\s+)?(persistent\\s+|temporary\\s+|temp\\s+)?secret\\b')\n\
         \x20           {writer_create_rule}\n\
         \x20           {writer_copy_rule}\n\
         \x20       )\n\
         \x20       WHEN 'reader' THEN (\n\
         \x20           _duckpack_auth.sanitize_sql(p_query) <> ''\n\
         \x20           AND strpos(_duckpack_auth.sanitize_sql(p_query), ';') = 0\n\
         \x20           AND regexp_matches(_duckpack_auth.sanitize_sql(p_query), '{reader_prefix_pattern}')\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(_duckpack_auth|quack_users|quack_sessions|quack_authentication_function|quack_authorization_function|quack_serve|quack_stop|quack_cancel)\\b')\n\
         \x20           AND NOT regexp_matches(_duckpack_auth.sanitize_sql(p_query), '\\b(insert|update|delete|merge|create|alter|drop|truncate|copy|call|vacuum|rename|grant|revoke|execute|attach|detach|install|load|export|import|set|reset)\\b')\n\
         \x20       )\n\
         \x20       ELSE false\n\
         \x20   END\n\
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

/// Writes a standalone server startup bootstrap SQL script to `project_dir/scripts/quack_rbac_init.sql`.
pub fn export_quack_rbac_bootstrap_script(
    config: &QuackAuthConfig,
    project_dir: &Path,
) -> Result<PathBuf> {
    let sql = compile_quack_auth_sql(config, project_dir, None, true)?;
    let scripts_dir = project_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir)?;
    let out_path = scripts_dir.join("quack_rbac_init.sql");
    std::fs::write(&out_path, sql)?;
    Ok(out_path)
}

/// Introspects the target database's `_duckpack_auth.quack_users` table and compares it against
/// `duckpack.toml` (`[quack_auth]`). If any user, role, token hash, or enabled flag has drifted,
/// or if `_duckpack_auth` has not been deployed yet, appends a declarative `DiffItem` to `diff_result`.
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
    let Ok(compiled_sql) = compile_quack_auth_sql(&config, project_dir, None, is_quack_target)
    else {
        return;
    };

    // Query existing _duckpack_auth.quack_users AND verify all 4 macros exist on target_exec
    let query = "SELECT username, role, token_sha256, client_id_hash, CAST(enabled AS VARCHAR) FROM _duckpack_auth.quack_users ORDER BY username";
    let macros_query = "SELECT count(DISTINCT function_name) FROM duckdb_functions() WHERE schema_name = '_duckpack_auth' AND function_name IN ('authenticate', 'authorize', 'resolve_role', 'sanitize_sql') AND (function_name <> 'authorize' OR COALESCE(macro_definition, '') LIKE '%use|pragma%')";
    let (actual_rows, macros_count) = if let Some(ref qa) = q_alias {
        let quoted_qa = format!("\"{}\"", qa.replace('"', "\"\""));
        let remote_q = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            query.replace('\'', "''")
        );
        let remote_m = format!(
            "SELECT * FROM {}.query('{}')",
            quoted_qa,
            macros_query.replace('\'', "''")
        );
        let rows = target_exec
            .query_rows(&remote_q)
            .or_else(|_| target_exec.query_rows(query))
            .ok()
            .map(|(_, rows)| rows);
        let m_count = target_exec
            .query_rows(&remote_m)
            .or_else(|_| target_exec.query_rows(macros_query))
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        (rows, m_count)
    } else {
        let rows = target_exec.query_rows(query).ok().map(|(_, rows)| rows);
        let m_count = target_exec
            .query_rows(macros_query)
            .ok()
            .and_then(|(_, r)| r.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        (rows, m_count)
    };

    let mut sorted_expected = expected_users.clone();
    sorted_expected.sort_by(|a, b| a.0.username.cmp(&b.0.username));

    let is_in_sync = if macros_count < 4 {
        false
    } else if let Some(ref rows) = actual_rows {
        if rows.len() != sorted_expected.len() {
            false
        } else {
            rows.iter().zip(sorted_expected.iter()).all(|(r, (u, th, ch))| {
                r.len() >= 5
                    && r[0] == u.username
                    && r[1] == u.role.as_str()
                    && r[2] == *th
                    && r[3] == *ch
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
            default_role: QuackRole::Reader,
            writer_allow_temp_tables: true,
            reader_allow_explain_describe: true,
            allow_copy: false,
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
}
