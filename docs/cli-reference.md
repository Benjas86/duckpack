[← Docs Index](./index.md)

# CLI Reference

Complete reference for the `duckpack` command-line interface.

---

## Global Options

| Flag | Description |
| :--- | :--- |
| `-e, --env <PROFILE>` | Load environment profile from `duckpack.toml` (`[env.<profile>]`, `[[env.<profile>.secrets]]`, `[[env.<profile>.attach]]`) and overlay `.env.<profile>` on top of `.env`. |
| `-V, --version` | Print DuckPack version and embedded DuckDB engine version (`v0.3.5 (DuckDB engine: v2.0.0)`). |
| `-h, --help` | Print help information for any command or subcommand. |

---

## Subcommands

### `duckpack init`
Scaffold a new declarative DuckDB project directory (`schemas/`, `sequences/`, `tables/`, `views/`, `macros/`, `triggers/`, `queries/`, `seeds/`, `scripts/`, `duckpack.toml`).

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Directory to initialize. |

---

### `duckpack build`
Validate all project SQL syntax, macros, sequences, and dependencies against an isolated in-memory Shadow DB.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |

---

### `duckpack diff`
Compare declarative project state against a target database without applying changes.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |
| `-d, --db <URI>` | `local.duckdb` | Target `.duckdb` path, `quack:host:port`, `md:<db>`, `ducklake:...`, or `mssql:...`. |
| `--database, --catalog <NAME>` | `None` | Target attached catalog name when multiple databases exist. |
| `-i, --interactive` | `false` | Open the interactive diff review TUI. |
| `--quack-token <TOKEN>` | `None` | Quack auth token (prefer `.env` `QUACK_TOKEN` or `token_env`). |
| `--quack-disable-ssl, --insecure` | `false` | Disable TLS for plain-HTTP Quack endpoints. |

---

### `duckpack apply`
Compute schema diffs and apply declarative migrations to the target database. Launches the interactive TUI review screen by default unless `--auto-approve` or `--rbac-only` is passed.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |
| `-d, --db <URI>` | `local.duckdb` | Target database path or URI. |
| `--database, --catalog <NAME>` | `None` | Target attached catalog name. |
| `--auto-approve` | `false` | Apply non-interactively (for CI/CD pipelines). |
| `-i, --interactive` | `false` | Launch interactive TUI review screen. |
| `--force-drop` | `false` | Permit destructive drops (`DropTable`, `DropColumn`, `DropView`, `DropMacro`, `DropTrigger`, `DropSequence`, `DropSchema`) and copy-and-swap table recreations (`RecreateTable`). |
| `--allow-lake-rewrite` | `false` | Permit physical Parquet copy-and-swap table rewrites on DuckLake catalogs (also via `DUCKPACK_ALLOW_LAKE_REWRITE=true`). |
| `--rbac-only` | `false` | Sync **only** `[quack_auth]` RBAC tables, salted `client_id_hash` values, and global Quack hooks without running user DDL migrations. |
| `--out <FILE>` | `None` | Dry-run mode: write computed migration SQL to `<FILE>` instead of executing. |
| `--quack-token <TOKEN>` | `None` | Quack auth token. |
| `--quack-disable-ssl, --insecure` | `false` | Disable TLS for Quack connections. |

---

### `duckpack explore`
Launch the interactive Terminal IDE. When `[[attach]]` is configured in `duckpack.toml` and no `--db` or `[target].db` is set, starts in clean **in-memory Workspace Mode**.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |
| `-d, --db <URI>` | `local.duckdb` | Target database path or URI. |
| `--database, --catalog <NAME>` | `None` | Initial target catalog name. |
| `--remote <SSH_TARGET>` | `None` | Run `explore` against a remote SSH host (e.g. `user@10.0.0.5`). |
| `-P, --port <PORT>` | `22` | SSH port for `--remote`. |
| `--auto-install` | `false` | Auto-install `duckpack` binary on the remote SSH host if missing. |
| `--quack-token <TOKEN>` | `None` | Quack auth token. |
| `--quack-disable-ssl, --insecure` | `false` | Disable TLS for Quack connections. |

---

### `duckpack compile`
Package a declarative project into a single immutable `.duckpack` snapshot artifact.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |
| `-o, --out <FILE>` | *(Required)* | Output `.duckpack` file path. |

---

### `duckpack deploy`
Compile and deploy a declarative project to a remote server over SCP/SSH.

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-p, --project-dir <DIR>` | `.` | Project directory path. |
| `-r, --remote <SSH_TARGET>` | *(Required)* | Remote SSH target (`user@host`). |
| `-P, --port <PORT>` | `22` | SSH port. |
| `-d, --db <URI>` | `local.duckdb` | Target database path on the remote server. |
| `--auto-install` | `false` | Auto-install `duckpack` binary on the remote host if missing. |

---

### `duckpack python`
Inspect or install the `duckpack` Python package in your active virtual environment (`$VIRTUAL_ENV`, `./.venv`, or `python3`).

- `duckpack python status` — Reports Python path, detected package manager (`uv` or `pip`), and installed versions.
- `duckpack python install [-U|--upgrade] [--installer auto|uv|pip] [--uv] [--pip]` — Installs or upgrades `duckpack` in the active environment.

---

## Environment Variables Reference

| Variable | Purpose |
| :--- | :--- |
| `QUACK_TOKEN` / `DUCKDB_QUACK_TOKEN` | Default authentication token for remote Quack connections. |
| `DUCKDB_QUACK_DISABLE_SSL` | Set `true` to disable TLS verification for Quack connections by default. |
| `DUCKPACK_ALLOW_LAKE_REWRITE` | Set `true` to permit physical copy-and-swap rewrites on DuckLake catalogs. |
| `DUCKPACK_S3_BUCKET` / `DUCKPACK_S3_BUCKETS` | Comma-separated cloud bucket URIs (e.g. `s3://lake,r2://analytics`) surfaced in Explorer's **Secrets & Cloud** drawer. |
| `DUCKPACK_ENV` | Optional environment profile name used by exported post-start scripts (`scripts/quack_rbac_post_start.sh`). |
| Custom `*_env` variables | Any variable referenced via `token_env`, `password_env`, `secret_env`, `key_id_env`, `dsn_env`, `endpoint_env`, `region_env`, `username_env`, or `${VAR}` SQL templates. |

---

## Embedded Engine Diagnostics

DuckPack embeds DuckDB v2.0 directly in-process. If an older database file with an incompatible pre-v1.0 storage format is opened, DuckPack intercepts the low-level engine error and outputs clear step-by-step migration instructions (including `EXPORT DATABASE` / `IMPORT DATABASE` or serving via `duckdb <file> -serve`).
