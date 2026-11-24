use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Validates that an `--env <profile>` name contains only safe identifier characters (`[A-Za-z0-9_-]`)
/// so it cannot traverse outside `project_dir` or inject shell metacharacters into generated scripts.
pub fn validate_safe_env_profile(profile: &str) -> Result<()> {
    let trimmed = profile.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!(
            "Invalid environment profile '{}': only ASCII letters, digits, '_' and '-' are permitted.",
            profile
        );
    }
    Ok(())
}

/// Strips control characters (`\r`, `\n`, `\0`) from a value before writing to `.env` to prevent newline injection.
pub fn sanitize_env_value(val: &str) -> String {
    val.chars()
        .filter(|&c| c != '\r' && c != '\n' && c != '\0')
        .collect()
}

/// Returns `true` if an environment variable name looks like a secret/credential
/// that should never be baked into compiled `.duckpack` SQL artifacts (H4).
pub fn is_sensitive_env_var_name(var_name: &str) -> bool {
    let upper = var_name.trim().to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper.ends_with("_PWD")
        || upper == "PWD_DB"
        || upper.contains("API_KEY")
        || upper.contains("ACCESS_KEY")
        || upper.contains("PRIVATE_KEY")
        || upper.contains("CREDENTIAL")
        || upper.contains("CONNECTION_STRING")
        || upper.ends_with("_DSN")
}

/// Writes `.env` (or sensitive secret/SQL script) contents to disk atomically with `0600`
/// permissions (owner read/write only) on Unix, avoiding any `0644` TOCTOU window.
pub fn write_restricted_env_file(env_path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(env_path)
            .with_context(|| format!("Failed to open {} for writing", env_path.display()))?;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        f.write_all(content.as_bytes())?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        std::fs::write(env_path, content)?;
        Ok(())
    }
}

/// Generates a cryptographically random 256-bit (64-hex-character) token using the OS CSPRNG
/// (`getrandom` across Linux, macOS, and Windows — zero non-cryptographic fallback).
pub fn generate_random_hex_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG (getrandom) failed to generate token bytes");
    let mut hex = String::with_capacity(64);
    for b in &bytes {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

/// Parses a `.env` file into a key/value map, supporting comments, blank lines, optional `export `
/// prefix, and single/double surrounding quotes consistently across all DuckPack modules.
pub fn parse_env_file_map(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
            if let Some((k, v)) = stripped.split_once('=') {
                let key = k.trim().to_string();
                let val = v.trim().trim_matches('"').trim_matches('\'').trim().to_string();
                if !key.is_empty() {
                    map.insert(key, val);
                }
            }
        }
    }
    map
}

/// Resolves an environment variable across the canonical precedence chain:
/// 1. Process environment (`std::env::var`)
/// 2. `project_dir/.env.<profile>` (when `project_dir` and a validated `env_profile` are given)
/// 3. `project_dir/.env` (when `project_dir` is given)
pub fn resolve_env_var(
    project_dir: Option<&Path>,
    env_profile: Option<&str>,
    key: &str,
) -> Option<String> {
    let k = key.trim();
    if k.is_empty() {
        return None;
    }
    if let Ok(val) = std::env::var(k) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let dir = project_dir?;
    if let Some(profile) = env_profile.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if validate_safe_env_profile(profile).is_ok() {
            let p = dir.join(format!(".env.{}", profile));
            if p.exists() {
                if let Some(v) = parse_env_file_map(&p).get(k).cloned() {
                    if !v.trim().is_empty() {
                        return Some(v);
                    }
                }
            }
        }
    }
    let base = dir.join(".env");
    if base.exists() {
        if let Some(v) = parse_env_file_map(&base).get(k).cloned() {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Loads environment variables from project directory:
/// 1. Always loads base `.env` if present.
/// 2. If `env_profile` is provided and passes `validate_safe_env_profile`, loads `.env.<profile>` on top.
pub fn load_env_files(project_dir: &Path, env_profile: Option<&str>) {
    let base_env = project_dir.join(".env");
    if base_env.exists() {
        let _ = dotenvy::from_path(&base_env);
    }
    if let Some(profile) = env_profile {
        if !profile.is_empty() && validate_safe_env_profile(profile).is_ok() {
            let profile_env = project_dir.join(format!(".env.{}", profile.trim()));
            if profile_env.exists() {
                let _ = dotenvy::from_path_override(&profile_env);
            }
        }
    }
}

/// Resolves the target `.env` file path for writing: returns `.env.<profile>` if `env_profile` is
/// set, valid, and already exists on disk; otherwise falls back to `.env`.
pub fn target_env_file_path(project_dir: &Path, env_profile: Option<&str>) -> Result<PathBuf> {
    if let Some(profile) = env_profile.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        validate_safe_env_profile(profile)?;
        let env_prof_path = project_dir.join(format!(".env.{}", profile));
        if env_prof_path.exists() {
            return Ok(env_prof_path);
        }
    }
    Ok(project_dir.join(".env"))
}

/// Atomically upserts `(key, value)` pairs into `.env` (or `.env.<profile>` if present) with `0600`
/// permissions and newline/control-character sanitization on both keys and values.
pub fn upsert_env_pairs(
    project_dir: &Path,
    env_profile: Option<&str>,
    pairs: &[(&str, &str)],
) -> Result<PathBuf> {
    let env_path = target_env_file_path(project_dir, env_profile)?;
    if pairs.is_empty() {
        return Ok(env_path);
    }

    let existing = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();

    for (raw_k, raw_v) in pairs {
        let k = sanitize_env_value(raw_k.trim());
        let v = sanitize_env_value(raw_v.trim());
        if k.is_empty() || k.contains('=') || v.is_empty() {
            continue;
        }
        let new_line = format!("{}={}", k, v);
        let mut updated = false;
        for line in &mut lines {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('#') {
                let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
                if let Some((existing_k, _)) = stripped.split_once('=') {
                    if existing_k.trim() == k {
                        *line = new_line.clone();
                        updated = true;
                        break;
                    }
                }
            }
        }
        if !updated {
            lines.push(new_line);
        }
    }

    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    write_restricted_env_file(&env_path, &out)?;
    Ok(env_path)
}

/// Safely writes or updates S3 / cloud credentials into the project's `.env` file (`0600`).
pub fn save_cloud_secret_to_env(
    project_dir: &Path,
    key_id: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
) -> Result<()> {
    let env_path = project_dir.join(".env");
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let mut keys_to_set = BTreeMap::new();
    let clean_key_id = sanitize_env_value(key_id);
    let clean_secret_key = sanitize_env_value(secret_key);
    let clean_region = sanitize_env_value(region);
    let clean_endpoint = sanitize_env_value(endpoint);
    let clean_bucket = sanitize_env_value(bucket);

    if !clean_key_id.is_empty() {
        keys_to_set.insert("AWS_ACCESS_KEY_ID", clean_key_id);
    }
    if !clean_secret_key.is_empty() {
        keys_to_set.insert("AWS_SECRET_ACCESS_KEY", clean_secret_key);
    }
    if !clean_region.is_empty() {
        keys_to_set.insert("AWS_REGION", clean_region.clone());
        keys_to_set.insert("AWS_DEFAULT_REGION", clean_region);
    }
    if !clean_endpoint.is_empty() {
        keys_to_set.insert("AWS_ENDPOINT", clean_endpoint);
    }
    if !clean_bucket.is_empty() {
        keys_to_set.insert("DUCKPACK_S3_BUCKET", clean_bucket);
    }

    let mut updated_keys = HashSet::new();
    for line in &mut lines {
        let key_opt = line.split_once('=').map(|(k, _)| k.trim().to_string());
        if let Some(trimmed_k) = key_opt {
            if let Some(new_val) = keys_to_set.get(trimmed_k.as_str()) {
                *line = format!("{}={}", trimmed_k, new_val);
                updated_keys.insert(trimmed_k);
            }
        }
    }

    for (k, v) in keys_to_set {
        if !updated_keys.contains(k) {
            lines.push(format!("{}={}", k, v));
        }
    }

    let output = lines.join("\n") + "\n";
    write_restricted_env_file(&env_path, &output)?;
    Ok(())
}

/// Safely appends or updates a cloud bucket URI into the project's `.env` file under `DUCKPACK_S3_BUCKETS`.
pub fn append_cloud_bucket_to_env(project_dir: &Path, bucket: &str) -> Result<()> {
    let env_path = project_dir.join(".env");
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let clean_bucket = sanitize_env_value(bucket.trim());
    let mut found = false;

    for line in &mut lines {
        let key_opt = line
            .split_once('=')
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()));
        if let Some((k, v)) = key_opt {
            if k == "DUCKPACK_S3_BUCKET" || k == "DUCKPACK_S3_BUCKETS" {
                let mut buckets: Vec<String> = v
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !buckets.contains(&clean_bucket) {
                    buckets.push(clean_bucket.clone());
                }
                *line = format!("DUCKPACK_S3_BUCKETS={}", buckets.join(","));
                found = true;
            }
        }
    }

    if !found {
        lines.push(format!("DUCKPACK_S3_BUCKET={}", clean_bucket));
    }

    let output = lines.join("\n") + "\n";
    write_restricted_env_file(&env_path, &output)?;
    Ok(())
}

/// Process-local, thread-safe store for runtime S3/cloud bucket URIs added or removed during
/// an IDE/workspace session — eliminates `unsafe std::env::set_var` mutations of `DUCKPACK_S3_BUCKETS`.
#[derive(Default)]
struct SessionBucketState {
    added: Vec<String>,
    removed: Vec<String>,
}

fn session_bucket_store() -> &'static std::sync::RwLock<SessionBucketState> {
    static STORE: std::sync::OnceLock<std::sync::RwLock<SessionBucketState>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::RwLock::new(SessionBucketState::default()))
}

fn bucket_matches(a: &str, b: &str) -> bool {
    let a_trim = a.trim();
    let b_trim = b.trim();
    a_trim.eq_ignore_ascii_case(b_trim)
        || a_trim.eq_ignore_ascii_case(b_trim.trim_start_matches("s3://"))
        || b_trim.eq_ignore_ascii_case(a_trim.trim_start_matches("s3://"))
}

/// Registers a cloud bucket URI in the active session view (thread-safe, no `std::env::set_var`).
pub fn add_session_cloud_bucket(bucket: &str) {
    let trimmed = bucket.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(mut guard) = session_bucket_store().write() {
        guard.removed.retain(|r| !bucket_matches(r, trimmed));
        if !guard.added.iter().any(|b| bucket_matches(b, trimmed)) {
            guard.added.push(trimmed.to_string());
        }
    }
}

/// Removes a cloud bucket URI from the active session view (including buckets inherited from env).
pub fn remove_session_cloud_bucket(bucket: &str) {
    let trimmed = bucket.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(mut guard) = session_bucket_store().write() {
        guard.added.retain(|b| !bucket_matches(b, trimmed));
        if !guard.removed.iter().any(|r| bucket_matches(r, trimmed)) {
            guard.removed.push(trimmed.to_string());
        }
    }
}

/// Replaces an existing cloud bucket URI in the active session view with a new URI.
pub fn replace_session_cloud_bucket(old_uri: &str, new_uri: &str) {
    remove_session_cloud_bucket(old_uri);
    add_session_cloud_bucket(new_uri);
}

/// Returns all active cloud bucket URIs by merging `DUCKPACK_S3_BUCKET` / `DUCKPACK_S3_BUCKETS`
/// from the environment with session-local additions/removals.
pub fn list_active_cloud_buckets() -> Vec<String> {
    let mut buckets = Vec::new();
    let guard = session_bucket_store().read().ok();
    let is_removed = |candidate: &str| -> bool {
        guard
            .as_ref()
            .map(|g| g.removed.iter().any(|r| bucket_matches(r, candidate)))
            .unwrap_or(false)
    };

    for var in ["DUCKPACK_S3_BUCKET", "DUCKPACK_S3_BUCKETS"] {
        if let Ok(val) = std::env::var(var) {
            for b in val.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                if !is_removed(b) && !buckets.iter().any(|e: &String| bucket_matches(e, b)) {
                    buckets.push(b.to_string());
                }
            }
        }
    }

    if let Some(g) = guard {
        for b in &g.added {
            if !buckets.iter().any(|e: &String| bucket_matches(e, b)) {
                buckets.push(b.clone());
            }
        }
    }

    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_store_profile_traversal_sanitization_and_csprng() {
        assert!(validate_safe_env_profile("prod").is_ok());
        assert!(validate_safe_env_profile("staging_eu-1").is_ok());
        assert!(validate_safe_env_profile("../../etc").is_err());
        assert!(validate_safe_env_profile("prod;id").is_err());

        assert!(is_sensitive_env_var_name("QUACK_TOKEN"));
        assert!(is_sensitive_env_var_name("AWS_SECRET_ACCESS_KEY"));
        assert!(is_sensitive_env_var_name("DB_PASSWORD"));
        assert!(!is_sensitive_env_var_name("ENV"));
        assert!(!is_sensitive_env_var_name("TENANT_SCHEMA"));

        let t1 = generate_random_hex_token();
        let t2 = generate_random_hex_token();
        assert_eq!(t1.len(), 64);
        assert_ne!(t1, t2);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));

        let dir = std::env::temp_dir().join(format!(
            "dp_env_store_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = upsert_env_pairs(&dir, None, &[("MY_KEY", "safe_val\nINJECTED=1")]).unwrap();
        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(!txt.contains("\nINJECTED=1"));
        assert_eq!(
            resolve_env_var(Some(&dir), None, "MY_KEY").as_deref(),
            Some("safe_valINJECTED=1")
        );
        // Reject traversal on profile write
        assert!(upsert_env_pairs(&dir, Some("../evil"), &[("K", "V")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_session_cloud_bucket_store_add_replace_remove() {
        let unique_a = "s3://dp-unit-test-bucket-alpha";
        let unique_b = "r2://dp-unit-test-bucket-beta";
        add_session_cloud_bucket(unique_a);
        assert!(list_active_cloud_buckets().iter().any(|b| b == unique_a));

        replace_session_cloud_bucket(unique_a, unique_b);
        let after_replace = list_active_cloud_buckets();
        assert!(!after_replace.iter().any(|b| b == unique_a));
        assert!(after_replace.iter().any(|b| b == unique_b));

        remove_session_cloud_bucket(unique_b);
        assert!(!list_active_cloud_buckets().iter().any(|b| b == unique_b));
    }
}
