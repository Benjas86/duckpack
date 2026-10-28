import os
import duckdb
import pytest
import duckpack

def test_duckpack_version_and_exports():
    assert duckpack.__version__ == "0.2.1_dev5"
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


def test_session_bridge_lifecycle_and_temp_detection():
    from duckpack import SessionBridge
    con = duckdb.connect()

    # When no temp tables/views, has_temp_objects is False
    assert SessionBridge.has_temp_objects(con) is False

    # Create temp table
    con.execute("CREATE TEMP TABLE t_test (id INT, name VARCHAR);")
    con.execute("INSERT INTO t_test VALUES (101, 'alpha');")
    assert SessionBridge.has_temp_objects(con) is True

    # Start SessionBridge
    bridge = SessionBridge(con, token="test_token")
    assert bridge.is_active is False
    assert bridge.token == "test_token"
    assert bridge.port > 0

    uri = bridge.start()
    assert uri.startswith("quack:127.0.0.1:")
    assert bridge.is_active is True
    assert bridge.db_uri == uri

    # Verify bridge tables staged
    staged = [r[0] for r in con.execute("SELECT table_name FROM duckdb_tables() WHERE database_name = 'memory' AND table_name LIKE '__duckpack_temp_bridge_%';").fetchall()]
    assert "__duckpack_temp_bridge_t_test" in staged

    # Stop bridge
    stopped = bridge.stop()
    assert stopped is True
    assert bridge.is_active is False

    # Stopping again is a safe no-op
    assert bridge.stop() is False

    # Bridge tables cleaned up
    remaining = [r[0] for r in con.execute("SELECT table_name FROM duckdb_tables() WHERE database_name = 'memory' AND table_name LIKE '__duckpack_temp_bridge_%';").fetchall()]
    assert len(remaining) == 0

    # Original temp table intact
    rows = con.execute("SELECT id, name FROM t_test;").fetchall()
    assert rows == [(101, 'alpha')]


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


def test_cli_version_and_subcommands(capsys, monkeypatch):
    from duckpack.cli import main
    import pytest

    # Test --version exits 0 and prints version string
    monkeypatch.setattr("sys.argv", ["duckpack", "--version"])
    with pytest.raises(SystemExit) as exc:
        main()
    assert exc.value.code == 0
    captured = capsys.readouterr()
    assert f"duckpack {duckpack.__version__}" in captured.out


def test_env_profile_with_base_env(tmp_path):
    proj_dir = str(tmp_path / "env_test_proj")
    duckpack.init(proj_dir)

    # Base .env contains QUACK_TOKEN
    with open(os.path.join(proj_dir, ".env"), "w") as f:
        f.write("QUACK_TOKEN=base_secret_999\nBASE_VAR=common\n")

    # duckpack.toml defines [target.dev] / [env.dev]
    with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
        f.write("""[project]
name = "env_proj"

[target]
db = "local.duckdb"

[env.dev]
host = "127.0.0.1"
port = 9494
""")

    # Resolving with env="dev" should pick up dev host/port AND base .env QUACK_TOKEN
    cfg = duckpack.get_target_config(project_dir=proj_dir, env="dev")
    assert cfg["db"] == "quack:127.0.0.1:9494"
    assert cfg["token"] == "base_secret_999"
    # Insecure is True because 127.0.0.1 is loopback
    import duckpack._duckpack as _dp
    # verify token resolution matches
    assert cfg["token"] == "base_secret_999"


def test_notebook_integration():
    from duckpack.notebook import (
        introspect_schema_summary,
        render_notebook_card,
        stop_server,
    )

    # 1. Test schema introspection
    con = duckdb.connect()
    con.execute("CREATE TABLE products (id INT, price DOUBLE, name VARCHAR);")
    con.execute("INSERT INTO products VALUES (1, 9.99, 'Widget'), (2, 19.99, 'Gadget');")
    summary = introspect_schema_summary(con)
    assert len(summary) >= 1
    prod = next(s for s in summary if s["name"] == "products")
    assert prod["columns"] == 3
    assert prod["rows"] == 2

    # 2. Test stop_server when no server active
    assert stop_server() is False


def test_render_notebook_card_in_notebook_environment(monkeypatch):
    from duckpack.notebook import render_notebook_card, stop_server

    rendered_html = []
    monkeypatch.setattr("duckpack.notebook.is_notebook_environment", lambda: True)
    monkeypatch.setattr("IPython.display.display", lambda obj: rendered_html.append(obj.data))

    con = duckdb.connect()
    con.execute("CREATE TABLE users (id INT, name VARCHAR);")
    res = render_notebook_card(con, project_dir=".", mode="explore")
    assert res is True
    assert "duckpack explore" in rendered_html[0]
    assert "DuckPack Database Explorer" in rendered_html[0]
    assert "users" in rendered_html[0]
    assert "Automatic terminal pop-out is disabled" in rendered_html[0]
    stop_server()


def test_interaction_runner_dispatch_hierarchy(monkeypatch):
    from duckpack.runner import InteractionRunner

    # 1. TTY priority
    monkeypatch.setattr("duckpack.runner.is_interactive_tty", lambda: True)
    res = InteractionRunner.dispatch(
        mode="explore",
        target="test.duckdb",
        on_tty=lambda: "ran_tty",
        on_headless=lambda: "ran_headless",
    )
    assert res == "ran_tty"

    # 2. Desktop Window emulator priority when not in TTY
    monkeypatch.setattr("duckpack.runner.is_interactive_tty", lambda: False)
    monkeypatch.setattr("duckpack.terminal.launch_terminal_window", lambda *args, **kwargs: True)
    res = InteractionRunner.dispatch(
        mode="apply",
        target="test.duckdb",
        open_window=True,
        on_window_applied=lambda: "window_applied",
        on_headless=lambda: "ran_headless",
    )
    assert res == "window_applied"

    # 3. Notebook fallback when window launcher returns False
    monkeypatch.setattr("duckpack.terminal.launch_terminal_window", lambda *args, **kwargs: False)
    monkeypatch.setattr("duckpack.notebook.is_notebook_environment", lambda: True)
    monkeypatch.setattr("duckpack.notebook.render_notebook_card", lambda *args, **kwargs: True)

    apply_card_res = InteractionRunner.dispatch(
        mode="apply",
        target="test.duckdb",
        open_window=True,
    )
    assert apply_card_res.success is True
    assert apply_card_res.applied_count == 0

    explore_card_res = InteractionRunner.dispatch(
        mode="explore",
        target="test.duckdb",
        open_window=True,
    )
    assert explore_card_res is None

    # 4. Headless fallback when not in notebook and window disabled
    monkeypatch.setattr("duckpack.notebook.is_notebook_environment", lambda: False)
    res = InteractionRunner.dispatch(
        mode="explore",
        target="test.duckdb",
        open_window=False,
        on_headless=lambda: "ran_headless_fallback",
    )
    assert res == "ran_headless_fallback"


def test_in_memory_connection_preserves_clean_session_without_local_duckdb(tmp_path):
    proj_dir = str(tmp_path)
    # Configure duckpack.toml with only default_db (standard local project setting)
    with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
        f.write('[project]\nname = "test_clean_proj"\n\n[target]\ndefault_db = "local.duckdb"\n')

    # Create dummy local.duckdb file
    local_db = duckdb.connect(os.path.join(proj_dir, "local.duckdb"))
    local_db.execute("CREATE TABLE secret_local_table (x INT);")
    local_db.close()

    # User starts their own in-memory connection
    con = duckdb.connect()
    con.execute("CREATE TABLE user_session_tbl AS SELECT 1 AS id;")

    # explore against user connection should NOT auto-attach local.duckdb
    duckpack.explore(con, project_dir=proj_dir, open_window=False)

    attached = [r[0] for r in con.execute("SELECT database_name FROM duckdb_databases();").fetchall()]
    assert "local" not in attached
    assert "local.duckdb" not in attached
    assert "memory" in attached


def test_pk_diff_and_apply_idempotency_in_memory(tmp_path):
    proj_dir = str(tmp_path / "pk_proj")
    duckpack.init(proj_dir)

    # 1. Target table initially without a primary key
    con = duckdb.connect()
    con.execute("CREATE TABLE users (id INT, name VARCHAR);")
    con.execute("INSERT INTO users VALUES (1, 'Alice'), (2, 'Bob');")

    # 2. Project model adds PRIMARY KEY (id)
    with open(os.path.join(proj_dir, "tables", "users.sql"), "w") as f:
        f.write("CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR);")

    # 3. Diff should detect AddConstraint
    diff_res = duckpack.diff(con, proj_dir)
    assert diff_res.has_changes is True
    assert diff_res.change_count == 1
    assert any("users" in item for item in diff_res.items)

    # 4. Apply in-place
    apply_res = duckpack.apply(con, proj_dir)
    assert apply_res.success is True
    assert apply_res.applied_count == 1

    # 5. Verify constraint applied in live database
    constrs = con.execute("SELECT constraint_type, constraint_column_names FROM duckdb_constraints() WHERE table_name = 'users';").fetchall()
    assert any(c[0] == "PRIMARY KEY" and c[1] == ["id"] for c in constrs)

    # 6. Re-diff MUST report zero changes (idempotency check)
    post_diff = duckpack.diff(con, proj_dir)
    assert post_diff.has_changes is False
    assert post_diff.change_count == 0


def test_constraint_copy_and_swap_migration_in_memory(tmp_path):
    proj_dir = str(tmp_path / "swap_proj")
    duckpack.init(proj_dir)

    # 1. Target table initially without CHECK constraint
    con = duckdb.connect()
    con.execute("CREATE TABLE orders (order_id INT PRIMARY KEY, amount DOUBLE);")
    con.execute("INSERT INTO orders VALUES (101, 49.99), (102, 99.50);")

    # 2. Project model adds CHECK (amount > 0)
    with open(os.path.join(proj_dir, "tables", "orders.sql"), "w") as f:
        f.write("CREATE TABLE orders (order_id INT PRIMARY KEY, amount DOUBLE, CHECK (amount > 0));")

    # 3. Diff should detect RecreateTable
    diff_res = duckpack.diff(con, proj_dir)
    assert diff_res.has_changes is True
    assert any("orders" in item for item in diff_res.items)

    # 4. Apply with force_drop=True to execute copy-and-swap
    apply_res = duckpack.apply(con, proj_dir, force_drop=True)
    assert apply_res.success is True

    # 5. Verify data is preserved after recreation
    rows = con.execute("SELECT order_id, amount FROM orders ORDER BY order_id;").fetchall()
    assert rows == [(101, 49.99), (102, 99.50)]

    # 6. Verify CHECK constraint exists
    constrs = con.execute("SELECT constraint_type FROM duckdb_constraints() WHERE table_name = 'orders';").fetchall()
    assert any(c[0] == "CHECK" for c in constrs)

    # 7. Re-diff MUST report zero changes
    post_diff = duckpack.diff(con, proj_dir)
    assert post_diff.has_changes is False


@pytest.mark.integration
@pytest.mark.skipif(not os.environ.get("QUACK_TOKEN"), reason="QUACK_TOKEN environment variable not set for live Quack test")
def test_live_quack_pk_diff_and_apply_idempotency(tmp_path):
    token = os.environ.get("QUACK_TOKEN")
    # Connect to live Quack container
    con = duckdb.connect()
    try:
        con.execute("LOAD quack;")
        con.execute(f"CREATE SECRET (TYPE QUACK, TOKEN '{token}');")
        con.execute("ATTACH 'quack:0.0.0.0:9494' AS target_db (DISABLE_SSL true);")
    except Exception as e:
        pytest.skip(f"Live Quack container not reachable: {e}")


    test_tbl = "__duckpack_pk_isolated_test"
    try:
        # Pre-create table on Quack without PK
        con.execute(f"CALL target_db.query('CREATE OR REPLACE TABLE {test_tbl} (id INT, val VARCHAR);');")

        proj_dir = str(tmp_path / "quack_pk_proj")
        duckpack.init(proj_dir)

        # Ignore all user tables on Quack so test focuses cleanly on isolated test table
        with open(os.path.join(proj_dir, "duckpack.toml"), "w") as f:
            f.write(f'[project]\nname = "quack_pk_proj"\n\n[ignore]\nrules = ["users", "products", "test_tbl", "wide_pipeline_test", "*view*", "target_table"]\n')

        with open(os.path.join(proj_dir, "tables", f"{test_tbl}.sql"), "w") as f:
            f.write(f"CREATE TABLE {test_tbl} (id INT PRIMARY KEY, val VARCHAR);")

        # Verify diff detects constraint
        diff_before = duckpack.diff(con, proj_dir, catalog="target_db")
        assert any(test_tbl in i for i in diff_before.items), f"Expected diff to detect PK change for {test_tbl}"

        # Apply to live Quack
        apply_res = duckpack.apply(con, proj_dir, catalog="target_db")
        assert apply_res.success is True

        # Verify zero diff after apply on live Quack
        diff_after = duckpack.diff(con, proj_dir, catalog="target_db")
        tbl_items = [i for i in diff_after.items if test_tbl in i]
        assert len(tbl_items) == 0, f"Expected 0 diff items for {test_tbl}, got {tbl_items}"
    finally:
        try:
            con.execute(f"CALL target_db.query('DROP TABLE IF EXISTS {test_tbl};');")
        except Exception:
            pass

