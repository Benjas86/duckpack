[← Docs Index](./index.md)

# CI/CD & Production Deployment

DuckPack is built for deterministic, auditable, zero-handcrafted-migration deployments across local files, remote Quack servers, DuckLake catalogs, and SSH hosts.

---

## 1. Offline Validation & Dry-Run SQL Generation

### Validate Syntax in Pull Requests (`build`)
Compile your entire project against an ephemeral in-memory Shadow DB (no live database credentials needed):

```bash
duckpack build --project-dir ./warehouse
```

### Detect Drift (`diff`)
Inspect pending changes against staging or production without modifying anything:

```bash
duckpack diff --project-dir ./warehouse --env prod
```

### Generate Reviewable Migration SQL (`--out`)
Emit the exact computed migration SQL script to disk for DBA or PR review instead of executing it:

```bash
duckpack apply --project-dir ./warehouse --env prod --out migration_plan.sql
```

---

## 2. Non-Interactive Production Migrations (`--auto-approve`)

```bash
# Safe additive & non-destructive migrations only (drops & table swaps are skipped)
duckpack apply --project-dir ./warehouse --env prod --auto-approve

# Allow destructive drops & copy-and-swap table recreations
duckpack apply --project-dir ./warehouse --env prod --auto-approve --force-drop

# Target a specific attached catalog
duckpack apply --project-dir ./warehouse --env prod --database analytics --auto-approve

# DuckLake catalog: explicitly authorize physical Parquet copy-and-swap rewrite
duckpack apply --project-dir ./warehouse --env lakehouse --auto-approve --force-drop --allow-lake-rewrite

# Quack server container restart: sync RBAC hooks & salted hashes ONLY (skip user DDL)
duckpack apply --project-dir ./warehouse --env prod --rbac-only --auto-approve
```

---

## 3. Immutable `.duckpack` Snapshot Artifacts (`compile`)

For air-gapped environments, release artifacts, or deterministic CI promotions, compile your declarative project into a single self-contained `.duckpack` snapshot (a pre-compiled DuckDB file with all `${VAR}` templates resolved):

```bash
duckpack compile --project-dir ./warehouse --out release_v1.2.0.duckpack
```

All schemas, sequences, macros, tables, views, and triggers are compiled in topological order and stored cleanly in the snapshot file.

---

## 4. Remote Deployment & Exploration Over SSH

### One-Step SSH Deploy (`deploy`)
Compile and deploy a project to a remote Linux/macOS server over SCP/SSH:

```bash
duckpack deploy \
  --project-dir ./warehouse \
  --remote deploy@prod-server.internal \
  --port 22 \
  --db /var/lib/duckdb/warehouse.duckdb \
  --auto-install
```

- `--auto-install` automatically downloads the matching `duckpack` binary onto the remote host if it isn't already installed.

### Remote Terminal IDE Over SSH (`explore --remote`)
Launch the full interactive Terminal IDE directly against a database file living on a remote SSH host:

```bash
duckpack explore \
  --remote deploy@prod-server.internal \
  --db /var/lib/duckdb/warehouse.duckdb \
  --auto-install
```

---

## 5. GitHub Actions Example

```yaml
name: DuckDB Schema CI/CD

on:
  pull_request:
    branches: [main]
  push:
    branches: [main]

jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install DuckPack
        run: |
          curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-linux-amd64 -o duckpack
          chmod +x duckpack
          sudo mv duckpack /usr/local/bin/

      - name: Validate SQL against Shadow DB
        run: duckpack build --project-dir ./warehouse

      - name: Compile release artifact
        run: duckpack compile --project-dir ./warehouse --out warehouse.duckpack

  deploy-prod:
    needs: validate
    if: github.ref == 'refs/heads/main' && github.event_name == 'push'
    runs-on: ubuntu-latest
    env:
      PROD_QUACK_TOKEN: ${{ secrets.PROD_QUACK_TOKEN }}
      S3_ACCESS_KEY: ${{ secrets.S3_ACCESS_KEY }}
      S3_SECRET_KEY: ${{ secrets.S3_SECRET_KEY }}
    steps:
      - uses: actions/checkout@v4

      - name: Install DuckPack
        run: |
          curl -L https://github.com/Benjas86/duckpack/releases/latest/download/duckpack-linux-amd64 -o duckpack
          chmod +x duckpack
          sudo mv duckpack /usr/local/bin/

      - name: Apply declarative migrations to production
        run: duckpack apply --project-dir ./warehouse --env prod --auto-approve
```
