import os
import duckdb
import pytest
import duckpack

def test_duckpack_version_and_exports():
    assert duckpack.__version__ == "0.2.0"
    assert hasattr(duckpack, "explore")
    assert hasattr(duckpack, "apply")
    assert hasattr(duckpack, "diff")
    assert hasattr(duckpack, "compile")
    assert hasattr(duckpack, "build")
    assert hasattr(duckpack, "init")
    assert issubclass(duckpack.SchemaError, duckpack.Error)
    assert issubclass(duckpack.ExecutionError, duckpack.Error)

def test_init_and_build(tmp_path):
    proj_dir = str(tmp_path / "my_proj")
    duckpack.init(proj_dir)

    assert os.path.exists(os.path.join(proj_dir, "tables"))
    assert os.path.exists(os.path.join(proj_dir, "schemas"))
    assert os.path.exists(os.path.join(proj_dir, "views"))
    assert os.path.exists(os.path.join(proj_dir, "macros"))
    assert os.path.exists(os.path.join(proj_dir, "duckpack.toml"))

    # Validate build on newly initialized project
    duckpack.build(proj_dir)

def test_diff_and_apply_in_memory(tmp_path):
    proj_dir = str(tmp_path / "db_proj")
    duckpack.init(proj_dir)

    # Add a table
    with open(os.path.join(proj_dir, "tables", "users.sql"), "w") as f:
        f.write("CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR, email VARCHAR);")

    # In-memory database
    con = duckdb.connect()

    # Diff should show new table
    diff_res = duckpack.diff(con, proj_dir)
    assert diff_res.has_changes is True
    assert diff_res.change_count == 1
    assert any("users" in item for item in diff_res.items)

    # Apply non-destructively
    apply_res = duckpack.apply(con, proj_dir)
    assert apply_res.success is True
    assert apply_res.applied_count == 1

    # Verify table in live connection
    tables = [t[0] for t in con.execute("SHOW TABLES;").fetchall()]
    assert "users" in tables

    # Verify columns
    cols = {row[0]: row[1] for row in con.execute("DESCRIBE users;").fetchall()}
    assert "id" in cols
    assert "name" in cols
    assert "email" in cols

    # Verify zero drift after apply
    post_diff = duckpack.diff(con, proj_dir)
    assert post_diff.has_changes is False

def test_safe_mode_prevents_destructive_drops(tmp_path):
    proj_dir = str(tmp_path / "safe_proj")
    duckpack.init(proj_dir)

    users_file = os.path.join(proj_dir, "tables", "users.sql")
    with open(users_file, "w") as f:
        f.write("CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR);")

    con = duckdb.connect()
    duckpack.apply(con, proj_dir)
    assert "users" in [t[0] for t in con.execute("SHOW TABLES;").fetchall()]

    # Delete users.sql from project
    os.remove(users_file)

    # Safe mode (force_drop=False) should NOT drop the table from the live DB
    duckpack.apply(con, proj_dir, force_drop=False)
    assert "users" in [t[0] for t in con.execute("SHOW TABLES;").fetchall()]

    # Force drop enables destructive drop
    duckpack.apply(con, proj_dir, force_drop=True)
    assert "users" not in [t[0] for t in con.execute("SHOW TABLES;").fetchall()]

def test_compile_artifact(tmp_path):
    proj_dir = str(tmp_path / "compile_proj")
    duckpack.init(proj_dir)

    with open(os.path.join(proj_dir, "tables", "products.sql"), "w") as f:
        f.write("CREATE TABLE products (sku VARCHAR PRIMARY KEY, price DOUBLE);")

    artifact_path = str(tmp_path / "release.duckpack")
    duckpack.compile(proj_dir, artifact_path)
    assert os.path.exists(artifact_path)

    # Verify artifact can be opened with DuckDB
    art_con = duckdb.connect(artifact_path)
    tables = [t[0] for t in art_con.execute("SHOW TABLES;").fetchall()]
    assert "products" in tables

def test_duckdbignore_rules(tmp_path):
    proj_dir = str(tmp_path / "ignore_proj")
    duckpack.init(proj_dir)

    with open(os.path.join(proj_dir, "tables", "active.sql"), "w") as f:
        f.write("CREATE TABLE active (id INT PRIMARY KEY);")

    with open(os.path.join(proj_dir, "tables", "temp_staging.sql"), "w") as f:
        f.write("CREATE TABLE temp_staging (val VARCHAR);")

    # Write ignore pattern
    with open(os.path.join(proj_dir, ".duckdbignore"), "w") as f:
        f.write("temp_*\n")

    con = duckdb.connect()
    # Diff should ignore temp_staging
    d = duckpack.diff(con, proj_dir)
    assert any("active" in item for item in d.items)
    assert not any("temp_staging" in item for item in d.items)

    # Apply should only create active
    duckpack.apply(con, proj_dir)
    tables = [t[0] for t in con.execute("SHOW TABLES;").fetchall()]
    assert "active" in tables
    assert "temp_staging" not in tables

def test_explore_non_tty_mode(capfd):
    con = duckdb.connect()
    con.execute("CREATE TABLE products (sku VARCHAR PRIMARY KEY, price DOUBLE);")
    con.execute("INSERT INTO products VALUES ('ABC', 19.99);")

    # explore in automated test/non-TTY environment should print formatted summary without error
    duckpack.explore(con)
    captured = capfd.readouterr()
    assert "DuckPack Database Explorer (Notebook / Non-TTY Mode)" in captured.out
    assert "products" in captured.out
    assert "sku" in captured.out

def test_in_memory_and_attached_catalog_targeting(tmp_path):
    proj_dir = str(tmp_path / "catalog_proj")
    duckpack.init(proj_dir)

    with open(os.path.join(proj_dir, "tables", "events.sql"), "w") as f:
        f.write("CREATE TABLE events (event_id INT, event_name VARCHAR);")

    con = duckdb.connect()

    # Explicitly specifying database="memory" on in-memory connection
    diff_mem = duckpack.diff(con, proj_dir, database="memory")
    assert diff_mem.has_changes is True
    assert diff_mem.change_count == 1

    apply_mem = duckpack.apply(con, proj_dir, database="memory")
    assert apply_mem.success is True
    assert apply_mem.applied_count == 1
    assert "events" in [t[0] for t in con.execute("SHOW TABLES;").fetchall()]

    # Attach secondary database 'analytics'
    con.execute("ATTACH '' AS analytics;")
    # Apply to attached secondary database
    apply_analytics = duckpack.apply(con, proj_dir, database="analytics")
    assert apply_analytics.success is True
    assert apply_analytics.applied_count == 1
    analytics_tables = [t[0] for t in con.execute("SHOW TABLES FROM analytics;").fetchall()]
    assert "events" in analytics_tables

def test_multi_catalog_toml_headless_apply(tmp_path):
    proj_dir = str(tmp_path / "multi_cat_proj")
    duckpack.init(proj_dir)

    # Configure multiple target catalogs in duckpack.toml
    with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
        f.write('[target]\ndatabase = ["memory", "quack_orders"]\n')

    with open(os.path.join(proj_dir, "tables", "users.sql"), "w") as f:
        f.write("CREATE TABLE users (id INT, email VARCHAR);")

    con = duckdb.connect()
    con.execute("ATTACH '' AS quack_orders;")

    # Headless apply with auto_approve=True without explicit database arg
    # duckpack must automatically resolve and migrate both configured catalogs!
    res = duckpack.apply(con, proj_dir, auto_approve=True)
    assert res.success is True
    assert res.applied_count == 2
    assert "users" in res.items

    # Verify both catalogs have the users table
    mem_tables = [t[0] for t in con.execute("SHOW TABLES FROM memory;").fetchall()]
    assert "users" in mem_tables

    orders_tables = [t[0] for t in con.execute("SHOW TABLES FROM quack_orders;").fetchall()]
    assert "users" in orders_tables

def test_explore_temporary_tables_and_views_in_non_tty(capfd):
    con = duckdb.connect()
    con.execute("CREATE TEMP TABLE staging_data (metric VARCHAR, val DOUBLE);")
    con.execute("INSERT INTO staging_data VALUES ('cpu', 78.5), ('mem', 42.0);")
    con.execute("CREATE TEMP VIEW staging_summary AS SELECT metric FROM staging_data;")

    duckpack.explore(con)
    captured = capfd.readouterr()
    assert "staging_data [temp] (2 rows)" in captured.out
    assert "metric" in captured.out
    assert "staging_summary [temp]" in captured.out

def test_diff_and_apply_ignore_temporary_tables(tmp_path):
    proj_dir = str(tmp_path / "temp_isolation_proj")
    duckpack.init(proj_dir)

    with open(os.path.join(proj_dir, "tables", "permanent.sql"), "w") as f:
        f.write("CREATE TABLE permanent (id INT PRIMARY KEY);")

    con = duckdb.connect()
    con.execute("CREATE TEMP TABLE temp_scratch (val INT);")

    diff_res = duckpack.diff(con, proj_dir)
    assert diff_res.has_changes is True
    # diff should only see the permanent table in the project vs the database
    # and must never propose dropping or migrating temp_scratch
    assert any("permanent" in item for item in diff_res.items)
    assert not any("temp_scratch" in item for item in diff_res.items)

    apply_res = duckpack.apply(con, proj_dir)
    assert apply_res.success is True

    # temp_scratch should still exist unharmed in session
    assert con.execute("SELECT * FROM temp_scratch;").fetchall() == []
    post_diff = duckpack.diff(con, proj_dir)
    assert post_diff.has_changes is False


def test_terminal_staging_and_sync_back_temp_objects():
    from duckpack.terminal import stage_connection_temp_objects, sync_back_connection_temp_objects
    con = duckdb.connect()
    con.execute("CREATE TEMP TABLE t_temp (x INT, y VARCHAR);")
    con.execute("INSERT INTO t_temp VALUES (1, 'apple'), (2, 'banana');")
    con.execute("CREATE TEMP VIEW v_temp AS SELECT y FROM t_temp WHERE x = 1;")

    stage_connection_temp_objects(con)

    # Verify staging bridge table in memory
    staged = [r[0] for r in con.execute("SELECT table_name FROM duckdb_tables() WHERE database_name = 'memory' AND table_name LIKE '__duckpack_temp_bridge_%';").fetchall()]
    assert "__duckpack_temp_bridge_t_temp" in staged

    # Simulate client modifying table and creating new one
    con.execute("CREATE OR REPLACE TABLE memory.__duckpack_temp_bridge_t_temp AS SELECT 3 AS x, 'cherry' AS y;")
    con.execute("CREATE OR REPLACE TABLE memory.__duckpack_temp_bridge_new_table AS SELECT 42 AS val;")

    sync_back_connection_temp_objects(con)

    # Verify restored changes in session
    rows = con.execute("SELECT x, y FROM t_temp;").fetchall()
    assert rows == [(3, 'cherry')]

    new_rows = con.execute("SELECT val FROM new_table;").fetchall()
    assert new_rows == [(42,)]

    # Cleaned up bridge tables
    remaining = [r[0] for r in con.execute("SELECT table_name FROM duckdb_tables() WHERE table_name LIKE '__duckpack_temp_bridge_%';").fetchall()]
    assert len(remaining) == 0


def test_quack_env_and_config_resolution(tmp_path):
    import subprocess
    proj_dir = str(tmp_path / "env_quack_proj")
    duckpack.init(proj_dir)

    # Verify .env generated with QUACK_TOKEN guidance
    with open(os.path.join(proj_dir, ".env"), "r") as f:
        env_content = f.read()
    assert "QUACK_TOKEN" in env_content

    # Verify duckpack.toml generated with remote Quack and token_env guidance
    with open(os.path.join(proj_dir, "duckpack.toml"), "r") as f:
        toml_content = f.read()
    assert "QUACK_TOKEN" in toml_content

    # Add a table to project
    with open(os.path.join(proj_dir, "tables", "metrics.sql"), "w") as f:
        f.write("CREATE TABLE metrics (id INT, value DOUBLE);")

    cli_bin = os.path.join(os.path.dirname(os.path.dirname(__file__)), "target", "debug", "duckpack")
    if os.path.exists(cli_bin):
        # 1. Test CLI security guardrail when --quack-token is used on command line
        res = subprocess.run(
            [cli_bin, "diff", "--project-dir", proj_dir, "--quack-token", "test_exposed_token"],
            capture_output=True,
            text=True
        )
        assert "Tip: Passing authentication tokens via CLI flags can expose credentials in shell history" in res.stderr

        # 2. Test [target] structured host and port fallback when --env is omitted
        with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
            f.write('[target]\nhost = "127.0.0.1"\nport = 9494\ninsecure = true\n')
        res2 = subprocess.run(
            [cli_bin, "diff", "--project-dir", proj_dir],
            capture_output=True,
            text=True
        )
        combined_err = res2.stderr + res2.stdout
        assert "quack:127.0.0.1:9494" in combined_err

        # 3. Test .env loading of QUACK_TOKEN eliminates "Could not find a Quack authentication token"
        with open(os.path.join(proj_dir, ".env"), "w") as f:
            f.write("QUACK_TOKEN=my_quack_secret\n")
        res3 = subprocess.run(
            [cli_bin, "diff", "--project-dir", proj_dir],
            capture_output=True,
            text=True
        )
        # With QUACK_TOKEN supplied, it executes CREATE SECRET and attempts connection, so the "Could not find a Quack authentication token" error is gone
        assert "Could not find a Quack authentication token" not in res3.stderr


def test_python_target_none_auto_resolution(tmp_path):
    proj_dir = str(tmp_path / "target_none_proj")
    duckpack.init(proj_dir)

    target_db_path = str(tmp_path / "target_none_proj" / "configured.duckdb")
    with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
        f.write(f'[project]\nname = "test"\n\n[target]\ndefault_db = "{target_db_path}"\n')

    with open(os.path.join(proj_dir, "tables", "products.sql"), "w") as f:
        f.write("CREATE TABLE products (sku VARCHAR PRIMARY KEY, price DOUBLE);\n")

    # 1. diff() with target=None should resolve to configured.duckdb
    diff_res = duckpack.diff(project_dir=proj_dir)
    assert diff_res.has_changes is True
    assert diff_res.change_count == 1
    assert any("products" in item for item in diff_res.items)

    # 2. apply() with target=None should apply to configured.duckdb
    apply_res = duckpack.apply(project_dir=proj_dir, auto_approve=True)
    assert apply_res.success is True
    assert apply_res.applied_count == 1

    # Verify table in configured.duckdb
    chk = duckdb.connect(target_db_path)
    tbls = [r[0] for r in chk.execute("SELECT table_name FROM duckdb_tables();").fetchall()]
    assert "products" in tbls
    chk.close()

    # 3. explore() with target=None and open_window=False should print non-terminal overview
    duckpack.explore(project_dir=proj_dir, open_window=False)


def test_python_connection_auto_attach_toml_target(tmp_path, capsys):
    proj_dir = str(tmp_path / "auto_attach_proj")
    duckpack.init(proj_dir)

    # Create a secondary external database to simulate analytical warehouse
    ext_db_path = str(tmp_path / "ext_warehouse.duckdb")
    ext_con = duckdb.connect(ext_db_path)
    ext_con.execute("CREATE TABLE orders (id INT, total DOUBLE);")
    ext_con.execute("INSERT INTO orders VALUES (101, 49.99), (102, 199.50);")
    ext_con.close()

    # Configure duckpack.toml with the external warehouse
    with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
        f.write(f'[project]\nname = "auto_attach_test"\n\n[target]\ndb = "{ext_db_path}"\ndatabase = "orders_warehouse"\n')

    # Create an in-memory session in Python with a local DataFrame/table
    con = duckdb.connect()
    con.execute("CREATE TABLE df_users (user_id INT, name VARCHAR);")
    con.execute("INSERT INTO df_users VALUES (101, 'Alice'), (102, 'Bob');")

    # Run explore against in-memory connection; should auto-attach orders_warehouse
    duckpack.explore(con, project_dir=proj_dir, open_window=False)

    captured = capsys.readouterr()
    assert "[DuckPack] Attached 'orders_warehouse'" in captured.out

    # Verify orders_warehouse is attached and accessible in con
    attached_dbs = [r[0] for r in con.execute("SELECT database_name FROM duckdb_databases();").fetchall()]
    assert "orders_warehouse" in attached_dbs

    # Verify cross-database joins work natively in Python
    joined = con.execute("""
        SELECT u.name, o.total
        FROM df_users u
        JOIN orders_warehouse.orders o ON u.user_id = o.id
        ORDER BY o.total ASC;
    """).fetchall()
    assert joined == [("Alice", 49.99), ("Bob", 199.50)]


