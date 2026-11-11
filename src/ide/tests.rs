use super::*;
use duckdb::types::Value;
use std::collections::HashMap;

#[test]
fn test_format_duckdb_value() {
    assert_eq!(format_duckdb_value(Value::Null), "NULL");
    assert_eq!(format_duckdb_value(Value::Boolean(true)), "true");
    assert_eq!(format_duckdb_value(Value::Int(42)), "42");
    assert_eq!(
        format_duckdb_value(Value::Text("hello".to_string())),
        "hello"
    );
}

#[test]
fn test_wide_table_column_windowing() {
    let cols: Vec<String> = (0..217).map(|i| format!("col_{}", i)).collect();
    let rows: Vec<Vec<String>> = vec![(0..217).map(|i| format!("val_{}", i)).collect()];

    let avail_width: usize = 80;
    let mut visible = Vec::new();
    let mut used = 0;
    for i in 0..cols.len() {
        let col_w = (cols[i].len().max(rows[0][i].len()) + 2).clamp(6, 30);
        if visible.is_empty() || used + col_w <= avail_width {
            used += col_w;
            visible.push(i);
        } else {
            break;
        }
    }
    assert!(!visible.is_empty());
    assert!(visible.len() < cols.len());
    assert_eq!(visible[0], 0);
    assert!(used <= avail_width);
}

#[test]
fn test_duckdb_detach() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
    conn.execute("USE target_db;", []).unwrap();
    // Switch to memory first, then detach
    conn.execute("USE memory;", []).unwrap();
    conn.execute("DETACH target_db;", []).unwrap();
    conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
    conn.execute("USE target_db;", []).unwrap();
}

#[test]
fn test_calculate_status_height() {
    // Single line short status
    assert_eq!(calculate_status_height("Ready.", 80, 30), 3);

    // Long status that wraps to 2 lines
    let long_msg = "A".repeat(100);
    assert_eq!(calculate_status_height(&long_msg, 80, 30), 4);

    // Multi-line status with newlines
    let multi_line = "Error: Line 1\nLine 2\nLine 3";
    assert_eq!(calculate_status_height(multi_line, 80, 30), 5);

    // Respects maximum limit clamped to total_height / 3 or 8
    let huge_msg = "Error:\n".repeat(20);
    assert_eq!(calculate_status_height(&huge_msg, 80, 30), 8);
}

#[test]
fn test_clipboard_roundtrip() {
    let mut cb: Option<arboard::Clipboard> = None;
    let test_str = "duckpack_test_clipboard_12345";
    assert!(set_clipboard_text(test_str, &mut cb));
    let retrieved = get_clipboard_text(&mut cb);
    if let Some(ref text) = retrieved {
        assert_eq!(text, test_str);
    }
}

#[test]
fn test_load_explorer_metadata_single_and_attached() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (id INTEGER, name VARCHAR);", [])
        .unwrap();
    conn.execute("CREATE VIEW active_users AS SELECT * FROM users;", [])
        .unwrap();

    let temp_dir = std::env::temp_dir().join(format!(
        "dp_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // 1. Single DB mode
    let meta_single = load_explorer_metadata(&conn, &temp_dir, false);
    assert_eq!(meta_single.databases.len(), 1);
    assert_eq!(meta_single.tables, vec!["users".to_string()]);
    assert_eq!(meta_single.views, vec!["active_users".to_string()]);

    // Rebuild single DB nodes: flat layout
    let mut expanded_groups = std::collections::HashSet::new();
    let mut expanded_tables = std::collections::HashSet::new();
    expanded_groups.insert("Tables".to_string());
    let nodes_single = rebuild_explorer_nodes(
        &meta_single,
        &expanded_groups,
        &expanded_tables,
        &conn,
        false,
    );
    assert!(
        nodes_single
            .iter()
            .any(|n| n.display.contains("Tables (1)"))
    );
    assert!(nodes_single.iter().any(|n| n.name == "users"));

    // 2. Attach a second database
    let attached_db_path = temp_dir.join("analytics.duckdb");
    conn.execute(
        &format!("ATTACH '{}' AS analytics;", attached_db_path.display()),
        [],
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE analytics.events (event_id BIGINT, event_name VARCHAR);",
        [],
    )
    .unwrap();

    let meta_multi = load_explorer_metadata(&conn, &temp_dir, false);
    assert_eq!(meta_multi.databases.len(), 2);
    assert!(
        meta_multi
            .databases
            .iter()
            .any(|d| d.name == "memory" && d.is_active)
    );
    assert!(
        meta_multi
            .databases
            .iter()
            .any(|d| d.name == "analytics" && !d.is_active)
    );

    // Rebuild multi-DB nodes: catalog-aware hierarchy
    expanded_groups.clear();
    expanded_tables.clear();
    expanded_groups.insert("db:analytics".to_string());
    expanded_groups.insert("analytics:Tables".to_string());
    expanded_tables.insert("analytics.main.events".to_string());

    let nodes_multi = rebuild_explorer_nodes(
        &meta_multi,
        &expanded_groups,
        &expanded_tables,
        &conn,
        false,
    );
    assert!(
        nodes_multi
            .iter()
            .any(|n| n.name == "db:analytics" && n.display.contains("analytics"))
    );
    assert!(nodes_multi.iter().any(|n| n.name == "analytics:Tables"));
    assert!(nodes_multi.iter().any(|n| n.name == "events"));
    assert!(nodes_multi.iter().any(|n| n.name == "event_id"));

    // 3. Schema visualizer with attached database
    let mut cb = None;
    let visualizer_result = generate_schema_visualizer(&conn, &mut cb, false);
    assert!(visualizer_result.is_ok());
    let tree_text = visualizer_result.unwrap();
    assert!(tree_text.contains("📁 memory"));
    assert!(tree_text.contains("📁 analytics"));
    assert!(tree_text.contains("events"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_quack_attach_preserves_target_db_and_memory() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
    conn.execute(
        "CREATE TABLE target_db.remote_tbl (col1 INTEGER, col2 VARCHAR);",
        [],
    )
    .unwrap();
    conn.execute("USE target_db;", []).unwrap();

    let temp_dir = std::env::temp_dir().join(format!(
        "dp_test_quack_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // When target_db is attached (e.g. Quack attach), both target_db and empty memory are preserved
    let meta = load_explorer_metadata(&conn, &temp_dir, false);
    assert_eq!(meta.databases.len(), 2);
    assert!(
        meta.databases
            .iter()
            .any(|d| d.name == "target_db" && d.is_active)
    );
    assert!(
        meta.databases
            .iter()
            .any(|d| d.name == "memory" && !d.is_active)
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_dot_autocomplete_word_extraction() {
    // Multi-part dot-qualified identifiers extraction
    let line = "SELECT analytics.main.events.event_id FROM t";
    let cursor_char_idx = 37; // right at the end of "event_id" (len of "SELECT analytics.main.events.event_id")
    let prefix: String = line.chars().take(cursor_char_idx).collect();
    let word_start = prefix
        .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    let current_word = &prefix[word_start..];
    assert_eq!(current_word, "analytics.main.events.event_id");

    let line2 = "SELECT db.";
    let prefix2: String = line2.chars().collect();
    let word_start2 = prefix2
        .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    let current_word2 = &prefix2[word_start2..];
    assert_eq!(current_word2, "db.");
}

#[test]
fn test_live_quack_metadata_loading() {
    let db_uri = "quack:0.0.0.0:9494";
    let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
    if token_var.is_empty() {
        return;
    }
    // Connect to live quack container if running
    let conn_res = crate::engine::connect_target_db(db_uri, Some(&token_var), true);
    if let Ok(conn) = conn_res {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_live_quack_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        conn.execute("CREATE TEMP TABLE t_test AS SELECT 1;", [])
            .unwrap();
        let meta = load_explorer_metadata(&conn, &temp_dir, true);
        println!(
            "Databases found: {:?}",
            meta.databases
                .iter()
                .map(|d| (&d.name, &d.db_type, d.is_active, d.tables.len()))
                .collect::<Vec<_>>()
        );
        assert_eq!(meta.databases.len(), 2, "Expected target_db and memory");
        assert!(
            meta.databases
                .iter()
                .any(|d| d.name == "target_db" && d.is_active)
        );
        assert!(
            meta.databases
                .iter()
                .any(|d| d.name == "memory" && !d.is_active)
        );

        let target_db = meta
            .databases
            .iter()
            .find(|d| d.name == "target_db")
            .unwrap();
        assert!(
            target_db.tables.iter().any(|t| t.name == "test_tbl"),
            "Expected test_tbl in target_db"
        );

        let mut expanded_groups = std::collections::HashSet::new();
        let expanded_tables = std::collections::HashSet::new();
        expanded_groups.insert("db:target_db".to_string());
        expanded_groups.insert("target_db:Tables".to_string());
        expanded_groups.insert("db:memory".to_string());

        let nodes =
            rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
        let node_displays: Vec<String> = nodes.iter().map(|n| n.display.clone()).collect();
        println!("Nodes rendered:\n{}", node_displays.join("\n"));

        assert!(
            nodes
                .iter()
                .any(|n| n.name == "db:target_db" && n.display.contains("target_db"))
        );
        assert!(
            nodes
                .iter()
                .any(|n| n.name == "db:memory" && n.display.contains("memory"))
        );
        assert!(nodes.iter().any(|n| n.name == "test_tbl"));

        let cols = load_table_columns_for("target_db", "main", "test_tbl", &conn, true);
        println!("Columns for test_tbl: {:?}", cols);
        assert!(cols.iter().any(|c| c.name == "column_a"));

        // Verify PK detection on live Quack for products table
        let prod_cols = load_table_columns_for("target_db", "main", "products", &conn, true);
        let pk_col = prod_cols
            .iter()
            .find(|c| c.name == "product_id")
            .expect("product_id not found in products");
        assert!(
            pk_col.is_pk,
            "product_id should be identified as primary key"
        );

        let mut expanded_tables = std::collections::HashSet::new();
        expanded_tables.insert("products".to_string());
        let nodes_with_cols =
            rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
        let pk_node = nodes_with_cols
            .iter()
            .find(|n| n.name == "product_id")
            .expect("product_id node not found");
        assert!(
            pk_node.display.contains("🔑 product_id") && pk_node.display.contains("[PK]"),
            "Expected key icon and [PK] tag, got: {}",
            pk_node.display
        );
        println!(
            "Verified live Quack Explorer PK rendering: {}",
            pk_node.display
        );

        // Verify FK detection and rendering on live Quack for orders table
        let order_cols = load_table_columns_for("target_db", "main", "orders", &conn, true);
        let user_id_col = order_cols
            .iter()
            .find(|c| c.name == "user_id")
            .expect("user_id not found in orders");
        assert!(
            user_id_col.is_fk,
            "user_id should be identified as foreign key"
        );
        assert_eq!(user_id_col.fk_target.as_deref(), Some("users.user_id"));

        expanded_tables.insert("orders".to_string());
        let nodes_with_orders =
            rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
        let fk_node = nodes_with_orders
            .iter()
            .find(|n| n.name == "user_id" && n.display.contains("[FK"))
            .expect("user_id FK node not found");
        assert!(
            fk_node.display.contains("🔗 user_id")
                && fk_node.display.contains("[FK -> users.user_id]"),
            "Expected FK icon and target, got: {}",
            fk_node.display
        );
        println!(
            "Verified live Quack Explorer FK rendering: {}",
            fk_node.display
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

#[test]
fn test_live_insert_users() {
    let db_uri = "quack:0.0.0.0:9494";
    let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
    if token_var.is_empty() {
        return;
    }
    if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_live_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
        let q = "insert into users values (9999, 'Test', 'User', '1900-01-01', 'test_user@example.com', now(), now(), True);";

        // Execute mutation via execute_batch
        let res = conn.execute_batch(q);
        assert!(res.is_ok(), "execute_batch failed: {:?}", res);

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM target_db.users WHERE user_id = 9999;",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "Expected exactly 1 row inserted, got {}", count);
        println!("Verified: Exactly 1 row inserted via execute_batch for users!");

        // Verify load_explorer_metadata returns the EXACT live count for users
        let meta = load_explorer_metadata(&conn, &temp_dir, true);
        let target_db = meta
            .databases
            .iter()
            .find(|d| d.name == "target_db")
            .unwrap();
        let users_table = target_db.tables.iter().find(|t| t.name == "users").unwrap();
        let actual_db_count: i64 = conn
            .query_row("SELECT count(*) FROM target_db.users;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            users_table.row_count,
            Some(actual_db_count),
            "Explorer metadata row_count must match actual count(*)"
        );
        println!(
            "Verified: Explorer metadata row count {} matches actual DB count {}",
            users_table.row_count.unwrap(),
            actual_db_count
        );

        let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

#[test]
fn test_temporary_tables_and_views_in_explorer() {
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_temp_tbl_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TEMP TABLE staging_tbl (id INT, label VARCHAR);", [])
        .unwrap();
    conn.execute(
        "INSERT INTO staging_tbl VALUES (1, 'alpha'), (2, 'beta');",
        [],
    )
    .unwrap();
    conn.execute(
        "CREATE TEMP VIEW staging_view AS SELECT id, label FROM staging_tbl;",
        [],
    )
    .unwrap();

    let meta = load_explorer_metadata(&conn, &temp_dir, false);
    assert!(meta.temp_tables.contains("staging_tbl"));
    assert!(meta.temp_views.contains("staging_view"));
    assert!(meta.tables.contains(&"staging_tbl".to_string()));
    assert!(meta.views.contains(&"staging_view".to_string()));

    let mut expanded_groups = HashSet::new();
    expanded_groups.insert("Tables".to_string());
    expanded_groups.insert("Views".to_string());
    let expanded_tables = HashSet::new();

    let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
    let tbl_node = nodes
        .iter()
        .find(|n| n.name == "staging_tbl")
        .expect("staging_tbl node not found");
    assert!(
        tbl_node.display.contains("[temp]"),
        "Expected [temp] in display: {}",
        tbl_node.display
    );
    match &tbl_node.node_type {
        NodeType::Table { is_temporary, .. } => assert!(*is_temporary),
        _ => panic!("Expected NodeType::Table"),
    }

    let view_node = nodes
        .iter()
        .find(|n| n.name == "staging_view")
        .expect("staging_view node not found");
    assert!(
        view_node.display.contains("[temp]"),
        "Expected [temp] in display: {}",
        view_node.display
    );
    match &view_node.node_type {
        NodeType::View { is_temporary, .. } => assert!(*is_temporary),
        _ => panic!("Expected NodeType::View"),
    }

    let cols =
        load_table_columns_for(&meta.active_database, "main", "staging_tbl", &conn, false);
    assert_eq!(cols.len(), 2);
    assert_eq!(cols[0].name, "id");
    assert_eq!(cols[1].name, "label");

    let view_cols =
        load_table_columns_for(&meta.active_database, "main", "staging_view", &conn, false);
    assert_eq!(view_cols.len(), 2);
    assert_eq!(view_cols[0].name, "id");
    assert_eq!(view_cols[1].name, "label");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_handle_sql_error_position_peg() {
    let mut textarea = TextArea::from(vec![
        "SELECT *".to_string(),
        "FROM non_existent".to_string(),
        "WHEREE x = 1".to_string(),
    ]);
    let err = "Parser Error: syntax error at line 3, column 1";
    handle_sql_error_position(&mut textarea, err);
    let (row, col) = textarea.cursor();
    assert_eq!(row, 2);
    assert_eq!(col, 0);
}

#[test]
fn test_handle_sql_error_position_legacy() {
    let mut textarea = TextArea::from(vec!["SELCT 1;".to_string()]);
    let err = "Parser Error: syntax error at or near \"SELCT\"";
    handle_sql_error_position(&mut textarea, err);
}

#[test]
fn test_triggers_in_explorer() {
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_trg_exp_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE orders (id INT, total DOUBLE);", [])
        .unwrap();
    conn.execute("CREATE TABLE order_audit (id INT);", [])
        .unwrap();
    conn.execute("CREATE TRIGGER trg_audit AFTER INSERT ON orders FOR EACH ROW INSERT INTO order_audit VALUES (NEW.id);", []).unwrap();

    let meta = load_explorer_metadata(&conn, &temp_dir, false);
    assert_eq!(meta.triggers.len(), 1);
    assert!(meta.triggers[0].contains("trg_audit"));

    let mut expanded_groups = HashSet::new();
    expanded_groups.insert("Triggers".to_string());
    let expanded_tables = HashSet::new();

    let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
    let trg_node = nodes
        .iter()
        .find(|n| n.name.contains("trg_audit"))
        .expect("trg_audit node not found");
    assert!(trg_node.display.contains("trg_audit"));
    match &trg_node.node_type {
        NodeType::Trigger { .. } => {}
        _ => panic!("Expected NodeType::Trigger"),
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_explorer_fk_and_pk_column_rendering() {
    // Direct test of format_column_display helper
    assert_eq!(
        format_column_display("  ", "id", "INTEGER", true, false, None),
        "  └─ 🔑 id (INTEGER) [PK]"
    );
    assert_eq!(
        format_column_display("  ", "parent_id", "INTEGER", false, true, Some("parent.id")),
        "  └─ 🔗 parent_id (INTEGER) [FK -> parent.id]"
    );
    assert_eq!(
        format_column_display("  ", "parent_id", "INTEGER", true, true, Some("parent.id")),
        "  └─ 🔑🔗 parent_id (INTEGER) [PK, FK -> parent.id]"
    );
    assert_eq!(
        format_column_display("  ", "name", "VARCHAR", false, false, None),
        "  └─ name (VARCHAR)"
    );

    // Integration test with database catalog and rebuild_explorer_nodes
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_fk_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE parent (id INT PRIMARY KEY, name VARCHAR);",
        [],
    )
    .unwrap();
    conn.execute("CREATE TABLE child (child_id INT PRIMARY KEY, parent_id INT REFERENCES parent(id), data VARCHAR);", []).unwrap();
    conn.execute("CREATE TABLE composite_child (parent_id INT REFERENCES parent(id), sub_id INT, PRIMARY KEY (parent_id, sub_id));", []).unwrap();

    let meta = load_explorer_metadata(&conn, &temp_dir, false);
    let mut expanded_groups = HashSet::new();
    expanded_groups.insert("Tables".to_string());
    let mut expanded_tables = HashSet::new();
    expanded_tables.insert("child".to_string());
    expanded_tables.insert("composite_child".to_string());

    let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

    // Check child table columns
    let child_pk = nodes
        .iter()
        .find(|n| n.name == "child_id")
        .expect("child_id not found");
    assert!(
        child_pk.display.contains("🔑 child_id"),
        "Expected 🔑 for child_id: {}",
        child_pk.display
    );
    assert!(
        child_pk.display.contains("[PK]"),
        "Expected [PK] for child_id: {}",
        child_pk.display
    );
    assert!(
        !child_pk.display.contains("🔗"),
        "child_id should not have FK icon"
    );

    let parent_id_nodes: Vec<_> = nodes.iter().filter(|n| n.name == "parent_id").collect();
    assert_eq!(
        parent_id_nodes.len(),
        2,
        "Expected 2 parent_id columns (child and composite_child)"
    );

    let child_fk = parent_id_nodes[0];
    assert!(
        child_fk.display.contains("🔗 parent_id"),
        "Expected 🔗 for parent_id in child: {}",
        child_fk.display
    );
    assert!(
        child_fk.display.contains("[FK -> parent.id]"),
        "Expected [FK -> parent.id]: {}",
        child_fk.display
    );
    assert!(
        !child_fk.display.contains("🔑"),
        "parent_id in child should not have PK icon"
    );

    // Check composite PK+FK table
    let composite_fk = parent_id_nodes[1];
    assert!(
        composite_fk.display.contains("🔑🔗 parent_id"),
        "Expected 🔑🔗 for composite parent_id: {}",
        composite_fk.display
    );
    assert!(
        composite_fk.display.contains("[PK, FK -> parent.id]"),
        "Expected [PK, FK -> parent.id]: {}",
        composite_fk.display
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_new_asset_modal_state() {
    let mut modal = NewAssetModalState::new();
    assert!(!modal.is_open);
    assert_eq!(modal.category_index, 0);
    assert_eq!(modal.current_category(), ProjectCategory::Table);
    assert_eq!(modal.category_label(), "Table");

    modal.reset();
    assert!(modal.is_open);
    assert_eq!(modal.active_field, 1);
    assert_eq!(modal.name, "");

    modal.category_index = 1;
    assert_eq!(modal.current_category(), ProjectCategory::View);
    assert_eq!(modal.category_label(), "View");

    modal.category_index = 2;
    assert_eq!(modal.current_category(), ProjectCategory::Macro);
    assert_eq!(modal.category_label(), "Macro");

    modal.category_index = 3;
    assert_eq!(modal.current_category(), ProjectCategory::Trigger);
    assert_eq!(modal.category_label(), "Trigger");

    modal.category_index = 4;
    assert_eq!(modal.current_category(), ProjectCategory::Sequence);
    assert_eq!(modal.category_label(), "Sequence");

    modal.category_index = 5;
    assert_eq!(modal.current_category(), ProjectCategory::Schema);
    assert_eq!(modal.category_label(), "Schema");

    modal.category_index = 6;
    assert_eq!(modal.current_category(), ProjectCategory::Query);
    assert_eq!(modal.category_label(), "Query");

    modal.category_index = 7;
    assert_eq!(modal.current_category(), ProjectCategory::Seed);
    assert_eq!(modal.category_label(), "Seed");
}

#[test]
fn test_generate_asset_boilerplate() {
    let table_sql = generate_asset_boilerplate(ProjectCategory::Table, "customers");
    assert!(table_sql.contains("CREATE TABLE customers"));
    assert!(table_sql.contains("id INTEGER PRIMARY KEY"));

    let view_sql = generate_asset_boilerplate(ProjectCategory::View, "active_users.sql");
    assert!(view_sql.contains("CREATE VIEW active_users AS"));

    let macro_sql = generate_asset_boilerplate(ProjectCategory::Macro, "to_cents");
    assert!(macro_sql.contains("CREATE MACRO to_cents(val)"));

    let trigger_sql = generate_asset_boilerplate(ProjectCategory::Trigger, "trg_audit");
    assert!(trigger_sql.contains("CREATE OR REPLACE TRIGGER trg_audit"));
    assert!(trigger_sql.contains("FOR EACH ROW"));

    let seq_sql = generate_asset_boilerplate(ProjectCategory::Sequence, "order_seq");
    assert!(seq_sql.contains("CREATE SEQUENCE IF NOT EXISTS order_seq"));

    let schema_sql = generate_asset_boilerplate(ProjectCategory::Schema, "analytics");
    assert!(schema_sql.contains("CREATE SCHEMA IF NOT EXISTS analytics"));

    let query_sql = generate_asset_boilerplate(ProjectCategory::Query, "adhoc_report");
    assert!(query_sql.contains("SELECT 1;"));

    let seed_data = generate_asset_boilerplate(ProjectCategory::Seed, "sample_data");
    assert!(seed_data.contains("id,name,value"));
}

#[test]
fn test_get_asset_file_path() {
    let root = PathBuf::from("/test/project");
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Schema, "staging"),
        root.join("schemas").join("staging.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Table, "orders.sql"),
        root.join("tables").join("orders.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::View, "v_orders"),
        root.join("views").join("v_orders.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Macro, "my_macro"),
        root.join("macros").join("my_macro.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Trigger, "orders_audit"),
        root.join("triggers").join("orders_audit.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Sequence, "orders_seq"),
        root.join("sequences").join("orders_seq.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Query, "analysis"),
        root.join("queries").join("analysis.sql")
    );
    assert_eq!(
        get_asset_file_path(&root, ProjectCategory::Seed, "countries"),
        root.join("seeds").join("countries.csv")
    );
}

#[test]
fn test_rebuild_project_nodes_and_git_badges() {
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_proj_nodes_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(temp_dir.join("tables")).unwrap();
    std::fs::create_dir_all(temp_dir.join("views")).unwrap();
    std::fs::create_dir_all(temp_dir.join("macros")).unwrap();

    let table_file = temp_dir.join("tables").join("users.sql");
    std::fs::write(&table_file, "CREATE TABLE users (id INT PRIMARY KEY);").unwrap();

    let view_file = temp_dir.join("views").join("v_users.sql");
    std::fs::write(&view_file, "CREATE VIEW v_users AS SELECT * FROM users;").unwrap();

    std::fs::write(
        temp_dir.join("duckpack.toml"),
        "[project]\nname = \"test\"\n",
    )
    .unwrap();

    let mut git_status = HashMap::new();
    git_status.insert(table_file.clone(), "[M]".to_string());
    git_status.insert(view_file.clone(), "[+]".to_string());

    let mut expanded = HashSet::new();
    expanded.insert("proj:tables".to_string());
    expanded.insert("proj:views".to_string());
    expanded.insert("proj:triggers".to_string()); // triggers directory doesn't exist on disk

    let nodes = rebuild_project_nodes(&temp_dir, &expanded, &git_status);

    // Verify tables group
    let table_group = nodes
        .iter()
        .find(|n| n.name == "proj:tables")
        .expect("proj:tables group not found");
    assert!(table_group.display.contains("Tables (1)"));

    // Verify users.sql with [M] badge
    let user_node = nodes
        .iter()
        .find(|n| n.name == "users.sql")
        .expect("users.sql node not found");
    assert!(user_node.display.contains("[M]"));

    // Verify views group
    let view_group = nodes
        .iter()
        .find(|n| n.name == "proj:views")
        .expect("proj:views group not found");
    assert!(view_group.display.contains("Views (1)"));

    // Verify v_users.sql with [+] badge
    let view_node = nodes
        .iter()
        .find(|n| n.name == "v_users.sql")
        .expect("v_users.sql node not found");
    assert!(view_node.display.contains("[+]"));

    // Verify triggers group is present even though triggers/ dir did not exist
    let trigger_group = nodes
        .iter()
        .find(|n| n.name == "proj:triggers")
        .expect("proj:triggers group not found");
    assert!(trigger_group.display.contains("Triggers (0)"));

    // Verify triggers empty placeholder node is present
    let empty_trigger_node = nodes
        .iter()
        .find(|n| n.name == "proj:triggers:empty")
        .expect("proj:triggers:empty placeholder not found");
    assert!(
        empty_trigger_node
            .display
            .contains("empty - press 'a' to add")
    );

    // Verify duckpack.toml config node
    let config_node = nodes
        .iter()
        .find(|n| n.name == "duckpack.toml")
        .expect("duckpack.toml node not found");
    assert!(config_node.display.contains("duckpack.toml"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_compute_and_apply_ide_diff() {
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_ide_diff_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(temp_dir.join("tables")).unwrap();

    let table_file = temp_dir.join("tables").join("items.sql");
    std::fs::write(
        &table_file,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, title VARCHAR);",
    )
    .unwrap();

    let target_conn = Connection::open_in_memory().unwrap();

    // 1. Compute diff (should detect items table needs to be created)
    let diff_result =
        compute_ide_diff(&temp_dir, &target_conn).expect("compute_ide_diff failed");
    assert_eq!(diff_result.items.len(), 1);
    assert_eq!(diff_result.items[0].obj_name, "items");
    assert_eq!(
        diff_result.items[0].item_type,
        crate::diff::DiffItemType::CreateTable
    );

    // 2. Apply diff in target_conn
    apply_ide_diff(&temp_dir, &target_conn, &diff_result).expect("apply_ide_diff failed");

    // 3. Recompute diff (should be 0 changes now)
    let diff_after = compute_ide_diff(&temp_dir, &target_conn).expect("recompute failed");
    assert_eq!(diff_after.items.len(), 0);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_live_quack_compute_and_apply_ide_diff() {
    let db_uri = "quack:0.0.0.0:9494";
    let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
    if token_var.is_empty() {
        return;
    }
    if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_quack_diff_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(temp_dir.join("tables"));

        // Ensure test table is dropped on remote
        let _ = conn.execute_batch(
            "CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');",
        );

        let table_file = temp_dir
            .join("tables")
            .join("__duckpack_ide_quack_test_tbl.sql");
        std::fs::write(&table_file, "CREATE TABLE __duckpack_ide_quack_test_tbl (id INTEGER PRIMARY KEY, note VARCHAR);").unwrap();

        // 1. Compute diff against live Quack connection
        let diff_result =
            compute_ide_diff(&temp_dir, &conn).expect("compute_ide_diff failed on Quack");
        let pending_item = diff_result
            .items
            .iter()
            .find(|i| i.obj_name == "__duckpack_ide_quack_test_tbl");
        assert!(
            pending_item.is_some(),
            "Expected pending create table for __duckpack_ide_quack_test_tbl"
        );

        // 2. Filter diff result to only apply our test table
        let mut test_diff = diff_result.clone();
        for item in &mut test_diff.items {
            if item.obj_name != "__duckpack_ide_quack_test_tbl" {
                item.selected = false;
            }
        }

        // 3. Apply diff via apply_ide_diff - this tests catalog resolution, transactions, and Quack DDL execution
        let apply_res = apply_ide_diff(&temp_dir, &conn, &test_diff);
        assert!(
            apply_res.is_ok(),
            "apply_ide_diff on Quack failed: {:?}",
            apply_res.err()
        );

        // 4. Verify table exists on remote Quack database
        let count: i64 = conn.query_row(
            "SELECT * FROM target_db.query('SELECT count(*) FROM __duckpack_ide_quack_test_tbl')",
            [],
            |r| r.get(0),
        ).expect("Failed to query created test table on remote Quack");
        assert_eq!(count, 0);

        // Cleanup remote table
        let _ = conn.execute_batch(
            "CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');",
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

#[test]
fn test_diff_space_toggle_and_lock_recomputation() {
    let mut diff = crate::diff::DiffResult {
        items: vec![
            crate::diff::DiffItem {
                item_type: crate::diff::DiffItemType::CreateTable,
                obj_name: "customers".to_string(),
                sql: "CREATE TABLE customers (id INT PRIMARY KEY);".to_string(),
                old_def: None,
                new_def: None,
                selected: true,
                locked: false,
                ignored: false,
            },
            crate::diff::DiffItem {
                item_type: crate::diff::DiffItemType::CreateView,
                obj_name: "v_customers".to_string(),
                sql: "CREATE VIEW v_customers AS SELECT * FROM customers;".to_string(),
                old_def: None,
                new_def: None,
                selected: true,
                locked: false,
                ignored: false,
            },
        ],
    };

    // Initially both are selected and unlocked
    assert!(diff.items[0].selected);
    assert!(!diff.items[0].locked);
    assert!(diff.items[1].selected);
    assert!(!diff.items[1].locked);

    // Toggle customers off with Space
    diff.items[0].selected = false;
    crate::tui::recompute_locks(&mut diff.items);

    // customers is unselected
    assert!(!diff.items[0].selected);
    assert!(!diff.items[0].locked);

    // Dependent view v_customers is automatically locked and deselected
    assert!(!diff.items[1].selected);
    assert!(diff.items[1].locked);

    // Re-selecting customers unlocks the view
    diff.items[0].selected = true;
    crate::tui::recompute_locks(&mut diff.items);
    assert!(diff.items[0].selected);
    assert!(!diff.items[1].locked);
}

#[test]
fn test_nested_schemas_in_ide_explorer() {
    let temp_dir = std::env::temp_dir().join(format!(
        "dp_ide_nested_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
    conn.execute("USE target_db;", []).unwrap();
    conn.execute("CREATE SCHEMA target_db.region;", []).unwrap();
    conn.execute("CREATE SCHEMA target_db.region.sales;", [])
        .unwrap();
    conn.execute(
        "CREATE TABLE target_db.region.products (id INT PRIMARY KEY);",
        [],
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE target_db.region.sales.orders (order_id INT PRIMARY KEY, qty INT);",
        [],
    )
    .unwrap();

    let meta = load_explorer_metadata(&conn, &temp_dir, false);
    let mut expanded_groups = HashSet::new();
    expanded_groups.insert("db:target_db".to_string());
    expanded_groups.insert("target_db:Tables".to_string());
    let expanded_tables = HashSet::new();

    let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

    // Verify products node display
    let prod_node = nodes
        .iter()
        .find(|n| n.name == "products")
        .expect("products node");
    assert!(prod_node.display.contains("region.products"));

    // Verify orders node display with nested schema
    let orders_node = nodes
        .iter()
        .find(|n| n.name == "orders")
        .expect("orders node");
    assert!(orders_node.display.contains("region.sales.orders"));

    match &orders_node.node_type {
        NodeType::Table {
            database, schema, ..
        } => {
            assert_eq!(database.as_deref(), Some("target_db"));
            assert_eq!(schema.as_deref(), Some("region.sales"));
        }
        _ => panic!("Expected NodeType::Table"),
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_attach_modal_state_providers() {
    let mut modal = AttachModalState::new();
    assert_eq!(modal.provider(), AttachProvider::DuckDB);
    assert!(!modal.is_open);

    modal.reset();
    assert!(modal.is_open);
    assert_eq!(modal.active_field, 1);

    modal.set_provider(1);
    assert_eq!(modal.provider(), AttachProvider::Quack);
    assert_eq!(modal.host, "localhost");
    assert_eq!(modal.port, "9494");
    assert_eq!(modal.alias, "quack_remote");

    modal.set_provider(2);
    assert_eq!(modal.provider(), AttachProvider::MotherDuck);

    modal.set_provider(3);
    assert_eq!(modal.provider(), AttachProvider::SQLite);

    modal.set_provider(4);
    assert_eq!(modal.provider(), AttachProvider::PostgreSQL);
    assert_eq!(modal.host, "localhost");
    assert_eq!(modal.port, "5432");
    assert_eq!(modal.username, "postgres");

    modal.set_provider(5);
    assert_eq!(modal.provider(), AttachProvider::MySQL);
    assert_eq!(modal.host, "localhost");
    assert_eq!(modal.port, "3306");
    assert_eq!(modal.username, "root");

    modal.set_provider(6);
    assert_eq!(modal.provider(), AttachProvider::MSSQL);
    assert_eq!(modal.host, "localhost");
    assert_eq!(modal.port, "1433");
    assert_eq!(modal.username, "sa");

    modal.set_provider(7);
    assert_eq!(modal.provider(), AttachProvider::DuckLake);
    assert_eq!(modal.ducklake_backend, DuckLakeBackend::Quack);
    assert_eq!(modal.host, "localhost");
    assert_eq!(modal.port, "9494");
    assert_eq!(modal.data_path, "s3://ducklake/data/");
    assert_eq!(modal.alias, "my_lakehouse");
    assert!(modal.disable_ssl);
    assert_eq!(modal.field_count(), 11);

    // Test DuckLake with Quack metadata catalog (default)
    let (quack_dl_target, quack_alias) = modal.build_connection_target().unwrap();
    assert_eq!(quack_alias, "my_lakehouse");
    assert_eq!(quack_dl_target, "ducklake:quack:localhost:9494");

    // Switch to PostgreSQL metadata catalog
    modal.set_ducklake_backend(DuckLakeBackend::PostgreSQL);
    assert_eq!(modal.port, "5432");
    assert_eq!(modal.username, "lakehouse");
    assert_eq!(modal.database, "ducklake");
    assert_eq!(modal.field_count(), 12);

    modal.password = "lakepass".to_string();
    let (dl_target, dl_alias) = modal.build_connection_target().unwrap();
    assert_eq!(dl_alias, "my_lakehouse");
    assert_eq!(
        dl_target,
        "ducklake:postgres:host=localhost port=5432 dbname=ducklake user=lakehouse password=lakepass"
    );

    // Switch to LocalFile backend
    modal.set_ducklake_backend(DuckLakeBackend::LocalFile);
    assert_eq!(modal.path, "metadata.ducklake");
    assert_eq!(modal.field_count(), 8);
    let (file_dl_target, _) = modal.build_connection_target().unwrap();
    assert_eq!(file_dl_target, "ducklake:metadata.ducklake");

    // Switch to SQLite backend
    modal.set_ducklake_backend(DuckLakeBackend::SQLite);
    assert_eq!(modal.path, "metadata.db");
    assert_eq!(modal.field_count(), 8);
    let (sqlite_dl_target, _) = modal.build_connection_target().unwrap();
    assert_eq!(sqlite_dl_target, "ducklake:sqlite:metadata.db");

    // Test cycling via toggle_active on field 1
    modal.active_field = 1;
    modal.toggle_active();
    assert_eq!(modal.ducklake_backend, DuckLakeBackend::Quack);
    assert_eq!(modal.field_count(), 11);

    modal.reset();
    modal.set_provider(6);

    // Test MSSQL Connection String building
    modal.database = "mydb".to_string();
    modal.alias = "mssql_db".to_string();
    modal.password = "secretpass".to_string();
    let (conn_str, alias) = modal.build_connection_target().unwrap();
    assert_eq!(alias, "mssql_db");
    assert!(conn_str.contains("Server=localhost,1433"));
    assert!(conn_str.contains("Database=mydb"));
    assert!(conn_str.contains("User Id=sa"));
    assert!(conn_str.contains("Password=secretpass"));
    assert!(conn_str.contains("TrustServerCertificate=yes"));

    modal.disable_ssl = true;
    let (conn_str_no_ssl, _) = modal.build_connection_target().unwrap();
    assert!(conn_str_no_ssl.contains("Encrypt=no"));

    // Test MSSQL with Windows Authentication / Trusted Connection
    modal.trusted_connection = true;
    let (trusted_conn, _) = modal.build_connection_target().unwrap();
    assert!(trusted_conn.contains("Trusted_Connection=yes"));
    assert!(!trusted_conn.contains("User Id="));
    assert!(!trusted_conn.contains("Password="));

    // Test toggling Windows Auth via Space
    modal.active_field = 8;
    modal.toggle_active(); // toggles off
    assert!(!modal.trusted_connection);
    assert_eq!(modal.username, "sa");

    modal.toggle_active(); // toggles back on
    assert!(modal.trusted_connection);
    assert_eq!(modal.username, "");

    // Test Postgres URI building
    modal.reset();
    modal.set_provider(4);
    modal.database = "analytics".to_string();
    modal.alias = "analytics".to_string();
    modal.password = "pgpass".to_string();
    modal.disable_ssl = true;
    let (pg_uri, pg_alias) = modal.build_connection_target().unwrap();
    assert_eq!(pg_alias, "analytics");
    assert_eq!(
        pg_uri,
        "postgres://postgres:pgpass@localhost:5432/analytics?sslmode=disable"
    );

    // Test MySQL URI building
    modal.reset();
    modal.set_provider(5);
    modal.database = "store".to_string();
    modal.alias = "store".to_string();
    modal.password = "mysqlpass".to_string();
    modal.disable_ssl = true;
    let (mysql_uri, mysql_alias) = modal.build_connection_target().unwrap();
    assert_eq!(mysql_alias, "store");
    assert_eq!(
        mysql_uri,
        "mysql://root:mysqlpass@localhost:3306/store?ssl-mode=DISABLED"
    );

    // Test Quack Target building
    modal.reset();
    modal.set_provider(1);
    let (quack_target, quack_alias) = modal.build_connection_target().unwrap();
    assert_eq!(quack_alias, "quack_remote");
    assert_eq!(quack_target, "quack:localhost:9494");

    // Test Typing and Backspace with auto-sync alias
    modal.reset();
    modal.set_provider(6);
    modal.active_field = 3; // Database Name field
    modal.handle_char('p');
    modal.handle_char('r');
    modal.handle_char('o');
    modal.handle_char('d');
    assert_eq!(modal.database, "prod");
    assert_eq!(modal.alias, "prod"); // Alias auto-populated
    modal.handle_backspace();
    assert_eq!(modal.database, "pro");
    assert_eq!(modal.alias, "pro");

    modal.active_field = 1; // Host field
    modal.host.clear();
    modal.handle_char('1');
    modal.handle_char('0');
    modal.handle_char('.');
    modal.handle_char('0');
    assert_eq!(modal.host, "10.0");
    modal.handle_backspace();
    assert_eq!(modal.host, "10.");

    // Test Navigation & Text Cursor Editing
    modal.active_field = 1; // Host field ("10.")
    assert_eq!(modal.cursor_pos, 3);
    let conn = Connection::open_in_memory().unwrap();
    // Left arrow moves cursor left
    modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Left, crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(modal.cursor_pos, 2);
    // Insert character '9' at cursor (between '0' and '.') -> "109."
    modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('9'), crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(modal.host, "109.");
    assert_eq!(modal.cursor_pos, 3);

    // Tab and BackTab field navigation
    modal.next_field();
    assert_eq!(modal.active_field, 2); // To Port
    modal.prev_field();
    assert_eq!(modal.active_field, 1); // Back to Host
    modal.nav_down();
    assert_eq!(modal.active_field, 3); // Down to Database Name

    modal.reset();
    assert!(!modal.disable_ssl);
    assert!(!modal.insecure);
    assert!(!modal.read_only);
}

#[test]
fn test_secret_modal_state_and_providers() {
    let mut modal = SecretModalState::new();
    assert_eq!(modal.provider(), CloudProvider::S3);
    assert_eq!(modal.region, "us-east-1");
    assert!(modal.use_ssl);
    assert!(modal.save_to_env);

    modal.set_provider(1); // Cloudflare R2
    assert_eq!(modal.provider(), CloudProvider::CloudflareR2);
    assert_eq!(modal.region, "auto");
    assert!(modal.endpoint.contains("r2.cloudflarestorage.com"));

    modal.set_provider(2); // MinIO
    assert_eq!(modal.provider(), CloudProvider::MinIO);
    assert_eq!(modal.endpoint, "localhost:9000");
    assert!(!modal.use_ssl);

    modal.reset();
    assert_eq!(modal.provider(), CloudProvider::S3);
    assert!(modal.is_open);
}

#[test]
fn test_explorer_nodes_secrets_and_cloud_buckets() {
    let conn = Connection::open_in_memory().unwrap();
    let meta = ExplorerMetadata {
        databases: vec![],
        active_database: "memory".to_string(),
        tables: vec![],
        table_counts: HashMap::new(),
        views: vec![],
        temp_tables: HashSet::new(),
        temp_views: HashSet::new(),
        macros: vec![],
        triggers: vec![],
        queries: vec![],
        dictionary: vec![],
        secrets: vec![("my_s3_secret".to_string(), "s3".to_string())],
        cloud_buckets: vec!["s3://analytics-lake".to_string()],
    };

    let mut expanded_groups = HashSet::new();
    let expanded_tables = HashSet::new();

    // Not expanded
    let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
    let sec_group = nodes
        .iter()
        .find(|n| n.name == "Secrets & Cloud Buckets")
        .expect("Secrets & Cloud Buckets group");
    assert!(sec_group.display.contains("Secrets & Cloud (2)"));
    assert!(!nodes.iter().any(|n| n.name == "my_s3_secret"));

    // Expanded
    expanded_groups.insert("Secrets & Cloud Buckets".to_string());
    let nodes_expanded =
        rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
    assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_secret"));
    assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_bucket"));
    assert!(nodes_expanded.iter().any(|n| n.name == "my_s3_secret"));
    assert!(
        nodes_expanded
            .iter()
            .any(|n| n.name == "bucket:s3://analytics-lake")
    );
}

#[test]
fn test_populate_secret_modal_for_edit() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TEMPORARY TABLE duckdb_secrets (name VARCHAR, secret_string VARCHAR);",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO duckdb_secrets VALUES ('test_edit', 'name=test_edit;endpoint=localhost:9000;key_id=my_key;use_ssl=false');", []).unwrap();

    let temp_dir = std::env::temp_dir().join(format!(
        "dp_ide_edit_sec_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&temp_dir);
    let _ = std::fs::write(
        temp_dir.join(".env"),
        "AWS_SECRET_ACCESS_KEY=my_secret_val\nDUCKPACK_S3_BUCKET=s3://test-bucket\n",
    );

    let mut modal = SecretModalState::new();
    populate_secret_modal_for_edit(&conn, &temp_dir, "test_edit", &mut modal);

    assert!(modal.is_open);
    assert_eq!(modal.name, "test_edit");
    assert_eq!(modal.key_id, "my_key");
    assert_eq!(modal.secret_key, "my_secret_val");
    assert_eq!(modal.endpoint, "localhost:9000");
    assert_eq!(modal.bucket, "s3://test-bucket");
    assert_eq!(modal.provider(), CloudProvider::MinIO);
    assert!(!modal.use_ssl);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_database_picker_modal_state() {
    let mut modal = DatabasePickerModalState::new();
    assert!(!modal.is_open);
    assert_eq!(modal.selected_index, 0);

    let dbs = vec![
        crate::catalog::DatabaseInfo {
            name: "memory".to_string(),
            db_type: "duckdb".to_string(),
            path: None,
            is_active: true,
            readonly: false,
            tables: vec![],
            views: vec![],
            triggers: vec![],
        },
        crate::catalog::DatabaseInfo {
            name: "analytics_db".to_string(),
            db_type: "postgres".to_string(),
            path: Some("postgres://...".to_string()),
            is_active: false,
            readonly: false,
            tables: vec![],
            views: vec![],
            triggers: vec![],
        },
        crate::catalog::DatabaseInfo {
            name: "mssql_db".to_string(),
            db_type: "mssql".to_string(),
            path: None,
            is_active: false,
            readonly: false,
            tables: vec![],
            views: vec![],
            triggers: vec![],
        },
    ];

    modal.open_for_current("analytics_db", &dbs);
    assert!(modal.is_open);
    assert_eq!(modal.selected_index, 1);

    modal.next(dbs.len());
    assert_eq!(modal.selected_index, 2);

    // Next at end should clamp to last element
    modal.next(dbs.len());
    assert_eq!(modal.selected_index, 2);

    modal.prev();
    assert_eq!(modal.selected_index, 1);

    modal.prev();
    assert_eq!(modal.selected_index, 0);

    // Prev at beginning should clamp to 0
    modal.prev();
    assert_eq!(modal.selected_index, 0);

    // Opening for unknown db falls back to index 0
    modal.open_for_current("unknown_catalog", &dbs);
    assert_eq!(modal.selected_index, 0);
}

#[test]
fn test_editor_tab_target_database_and_db_icon() {
    let tab = EditorTab::new("mssql_db".to_string());
    assert_eq!(tab.target_database, "mssql_db");
    assert!(!tab.is_dirty);
    assert!(tab.active_file.is_none());

    assert_eq!(db_icon("ducklake"), "🌊");
    assert_eq!(db_icon("sqlite"), "🗃️");
    assert_eq!(db_icon("postgres"), "🔌");
    assert_eq!(db_icon("postgresql"), "🔌");
    assert_eq!(db_icon("mysql"), "🔌");
    assert_eq!(db_icon("mssql"), "🔌");
    assert_eq!(db_icon("duckdb"), "📁");
    assert_eq!(db_icon("unknown"), "📁");
}

#[test]
fn test_exit_confirmation_modal_state() {
    let mut modal = ExitConfirmationModalState::new();
    assert!(!modal.is_open);
    assert!(!modal.confirm_exit);

    modal.open();
    assert!(modal.is_open);
    assert!(!modal.confirm_exit); // Safe default: Cancel

    modal.toggle();
    assert!(modal.confirm_exit); // Toggled to Exit

    modal.toggle();
    assert!(!modal.confirm_exit); // Toggled back to Cancel

    modal.close();
    assert!(!modal.is_open);
    assert!(!modal.confirm_exit);

    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    modal.open();
    assert_eq!(
        modal.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        None
    );
    assert!(modal.confirm_exit);
    assert_eq!(
        modal.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(true)
    );

    modal.open();
    assert_eq!(
        modal.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(false)
    );
    assert!(!modal.is_open);
}

#[test]
fn test_new_asset_modal_handle_key_and_editor_tab_load_content() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let temp_dir = std::env::temp_dir().join(format!(
        "duckpack_test_ide_modal_key_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let mut modal = NewAssetModalState::new();
    modal.reset();
    for ch in "orders".chars() {
        assert_eq!(
            modal.handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &temp_dir
            ),
            None
        );
    }
    let created = modal
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &temp_dir)
        .expect("Expected Enter to create orders.sql");
    assert!(created.exists());
    assert!(!modal.is_open);

    let mut tabs = vec![EditorTab::new("memory".to_string())];
    let mut active_idx = 0;
    assert!(tabs[0].is_untitled_empty());
    assert!(open_or_focus_file_in_tabs(
        &mut tabs,
        &mut active_idx,
        created.clone()
    ));
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].active_file.as_ref(), Some(&created));
    assert!(!tabs[0].is_untitled_empty());

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_pagination_page_sizes_and_explicit_sql_limit() {
    let mut tab = EditorTab::new("memory".to_string());
    assert_eq!(tab.pagination_limit, 2_000);
    assert_eq!(tab.pagination_limit_label(), "2000");

    tab.pagination_offset = 4_000;
    assert_eq!(tab.cycle_pagination_limit(), 10_000);
    assert_eq!(tab.pagination_offset, 0);
    assert_eq!(tab.pagination_limit_label(), "10000");

    assert_eq!(tab.cycle_pagination_limit(), 50_000);
    assert_eq!(tab.pagination_limit_label(), "50000");

    assert_eq!(tab.cycle_pagination_limit(), 0);
    assert_eq!(tab.pagination_limit_label(), "All");

    assert_eq!(tab.cycle_pagination_limit(), 500);
    assert_eq!(tab.pagination_limit_label(), "500");

    assert_eq!(tab.cycle_pagination_limit(), 2_000);

    // Explicit SQL LIMIT detection
    assert_eq!(
        extract_explicit_sql_limit("SELECT * FROM range(5000) LIMIT 1500;"),
        Some(1500)
    );
    assert_eq!(
        extract_explicit_sql_limit("SELECT * FROM range(5000) limit 1200 offset 10"),
        Some(1200)
    );
    assert_eq!(extract_explicit_sql_limit("SELECT * FROM users;"), None);

    // build_paginated_sql honours explicit LIMIT > pagination_limit without truncating to 500
    let (sql_explicit, is_pag_explicit) =
        build_paginated_sql("SELECT * FROM range(5000) LIMIT 1500;", 500, 0);
    assert_eq!(sql_explicit, "SELECT * FROM range(5000) LIMIT 1500");
    assert!(!is_pag_explicit);

    // Standard query wraps with pagination_limit and pagination_offset
    let (sql_wrapped, is_pag_wrapped) = build_paginated_sql("SELECT * FROM users;", 2_000, 0);
    assert_eq!(
        sql_wrapped,
        "SELECT * FROM (SELECT * FROM users) LIMIT 2000 OFFSET 0"
    );
    assert!(is_pag_wrapped);

    // Unlimited page size (0) executes directly without LIMIT wrapper
    let (sql_all, is_pag_all) = build_paginated_sql("SELECT * FROM users;", 0, 0);
    assert_eq!(sql_all, "SELECT * FROM users");
    assert!(!is_pag_all);
}

#[test]
fn test_explorer_column_data_type_colour_coding_and_scrolling() {
    assert_eq!(
        column_data_type_style("INTEGER").fg,
        Some(Color::LightGreen)
    );
    assert_eq!(
        column_data_type_style("DECIMAL(18, 2)").fg,
        Some(Color::LightGreen)
    );
    assert_eq!(column_data_type_style("VARCHAR").fg, Some(Color::Yellow));
    assert_eq!(column_data_type_style("UUID").fg, Some(Color::Yellow));
    assert_eq!(
        column_data_type_style("TIMESTAMP WITH TIME ZONE").fg,
        Some(Color::LightMagenta)
    );
    assert_eq!(column_data_type_style("BOOLEAN").fg, Some(Color::Magenta));
    let complex_style = column_data_type_style("STRUCT(a INT, b VARCHAR)");
    assert_eq!(complex_style.fg, Some(Color::LightBlue));
    assert!(complex_style.add_modifier.contains(Modifier::BOLD));

    let col_node = ExplorerNode {
        display: format_column_display(
            "  ",
            "user_id",
            "INTEGER",
            true,
            true,
            Some("users.id"),
        ),
        name: "user_id".to_string(),
        node_type: NodeType::Column {
            data_type: "INTEGER".to_string(),
            is_pk: true,
            is_fk: true,
            fk_target: Some("users.id".to_string()),
        },
        level: 2,
        is_expandable: false,
        is_expanded: false,
    };

    let line = render_explorer_node_line(&col_node, 0);
    let full_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(
        full_text,
        "      └─ 🔑🔗 user_id (INTEGER) [PK, FK -> users.id]"
    );

    let dt_span = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "INTEGER")
        .expect("Expected styled INTEGER span");
    assert_eq!(dt_span.style.fg, Some(Color::LightGreen));

    let pk_span = line
        .spans
        .iter()
        .find(|s| s.content.starts_with("[PK"))
        .expect("Expected styled [PK span");
    assert_eq!(pk_span.style.fg, Some(Color::Yellow));
    assert!(pk_span.style.add_modifier.contains(Modifier::BOLD));

    let fk_span = line
        .spans
        .iter()
        .find(|s| s.content.contains("FK -> users.id]"))
        .expect("Expected styled FK span");
    assert_eq!(fk_span.style.fg, Some(Color::Cyan));

    // Verify horizontal scroll preserves characters and span styles after skipping 6 leading spaces
    let scrolled_line = render_explorer_node_line(&col_node, 6);
    let scrolled_text: String = scrolled_line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        scrolled_text,
        "└─ 🔑🔗 user_id (INTEGER) [PK, FK -> users.id]"
    );
}

#[test]
fn test_exit_confirmation_modal_render_with_warning() {
    let backend = ratatui::backend::TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut modal = ExitConfirmationModalState::new();
    modal.open();

    terminal
        .draw(|f| {
            let rect = modal.render_with_warning(
                f,
                " ⚠️ Exit DuckPack Diff Review ",
                "Are you sure you want to exit Diff Review?",
                Some("⚠️ 3 pending change(s) have not been applied!"),
            );
            assert_eq!(rect.height, 9);
        })
        .unwrap();
}

#[test]
fn test_render_text_with_cursor_and_helpers() {
    use crate::ide::modals::{
        delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
        render_text_with_cursor,
    };

    let mut s = "hello".to_string();
    let mut cur = 2; // between 'e' and 'l'
    insert_char_at_cursor(&mut s, &mut cur, 'X');
    assert_eq!(s, "heXllo");
    assert_eq!(cur, 3);

    delete_char_before_cursor(&mut s, &mut cur);
    assert_eq!(s, "hello");
    assert_eq!(cur, 2);

    delete_char_at_cursor(&mut s, cur);
    assert_eq!(s, "helo"); // deleted first 'l'
    assert_eq!(cur, 2);

    // Rendering tests
    let line_end = render_text_with_cursor("abc", 3, true, false);
    assert_eq!(line_end.spans.len(), 2);
    assert_eq!(line_end.spans[0].content, "abc");
    assert_eq!(line_end.spans[1].content, "█");

    let line_mid = render_text_with_cursor("abc", 1, true, false);
    assert_eq!(line_mid.spans.len(), 3);
    assert_eq!(line_mid.spans[0].content, "a");
    assert_eq!(line_mid.spans[1].content, "b"); // cursor under 'b'
    assert_eq!(line_mid.spans[2].content, "c");

    let line_masked = render_text_with_cursor("secret", 6, true, true);
    assert_eq!(line_masked.spans[0].content, "******");
    assert_eq!(line_masked.spans[1].content, "█");

    let line_inactive = render_text_with_cursor("text", 2, false, false);
    assert_eq!(line_inactive.spans.len(), 1);
    assert_eq!(line_inactive.spans[0].content, "text");
}

#[test]
fn test_all_modals_cursor_navigation_and_editing() {
    let conn = Connection::open_in_memory().unwrap();
    let pdir = std::path::Path::new(".");

    // 1. NewAssetModalState
    let mut asset_modal = NewAssetModalState::new();
    asset_modal.category_index = 0; // SQL Table
    asset_modal.active_field = 1; // Name field
    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('u'), crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('s'), crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('r'), crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.name, "usr");
    assert_eq!(asset_modal.cursor_pos, 3);

    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Left, crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.cursor_pos, 2);
    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('e'), crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.name, "user");
    assert_eq!(asset_modal.cursor_pos, 3);

    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Home, crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.cursor_pos, 0);
    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Delete, crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.name, "ser");
    assert_eq!(asset_modal.cursor_pos, 0);

    asset_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::End, crossterm::event::KeyModifiers::NONE),
        pdir,
    );
    assert_eq!(asset_modal.cursor_pos, 3);

    // 2. AddBucketModalState
    let mut bucket_modal = AddBucketModalState::new();
    let mut exp_groups = HashSet::new();
    bucket_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('a'), crossterm::event::KeyModifiers::NONE),
        pdir,
        &mut exp_groups,
    );
    bucket_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('c'), crossterm::event::KeyModifiers::NONE),
        pdir,
        &mut exp_groups,
    );
    bucket_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Left, crossterm::event::KeyModifiers::NONE),
        pdir,
        &mut exp_groups,
    );
    bucket_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('b'), crossterm::event::KeyModifiers::NONE),
        pdir,
        &mut exp_groups,
    );
    assert_eq!(bucket_modal.uri, "abc");

    // 3. SecretModalState
    let mut sec_modal = SecretModalState::new();
    sec_modal.name.clear();
    sec_modal.sync_cursor_to_end();
    sec_modal.active_field = 1; // Secret Name
    sec_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('m'), crossterm::event::KeyModifiers::NONE),
        &conn,
        pdir,
    );
    sec_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('y'), crossterm::event::KeyModifiers::NONE),
        &conn,
        pdir,
    );
    assert_eq!(sec_modal.name, "my");
    sec_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Left, crossterm::event::KeyModifiers::NONE),
        &conn,
        pdir,
    );
    assert_eq!(sec_modal.cursor_pos, 1);
    sec_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('_'), crossterm::event::KeyModifiers::NONE),
        &conn,
        pdir,
    );
    assert_eq!(sec_modal.name, "m_y");

    // 4. AttachModalState
    let mut attach_modal = AttachModalState::new();
    attach_modal.set_provider(7); // DuckLake
    assert_eq!(attach_modal.provider(), AttachProvider::DuckLake);
    assert_eq!(attach_modal.ducklake_backend, DuckLakeBackend::Quack);

    // Backend switching on field 1
    attach_modal.active_field = 1;
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Right, crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(attach_modal.ducklake_backend, DuckLakeBackend::PostgreSQL);
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Left, crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(attach_modal.ducklake_backend, DuckLakeBackend::Quack);

    // DATA_PATH text editing on Quack DuckLake (field 6)
    attach_modal.active_field = 6;
    attach_modal.data_path.clear();
    attach_modal.sync_cursor_to_end();
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('s'), crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('3'), crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(attach_modal.data_path, "s3");
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Home, crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(attach_modal.cursor_pos, 0);
    attach_modal.handle_key(
        crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('/'), crossterm::event::KeyModifiers::NONE),
        &conn,
    );
    assert_eq!(attach_modal.data_path, "/s3");
}

#[test]
fn test_ducklake_modal_render_height_and_2d_nav() {
    let backend = ratatui::backend::TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut modal = AttachModalState::new();
    modal.is_open = true;
    modal.set_provider(7); // DuckLake Quack

    terminal
        .draw(|f| {
            modal.render(f);
        })
        .unwrap();

    // 2D navigation Quack:
    // 0 (Provider) down -> 2 (Host)
    modal.active_field = 0;
    modal.nav_down();
    assert_eq!(modal.active_field, 2);

    // 1 (Backend) down -> 3 (Port)
    modal.active_field = 1;
    modal.nav_down();
    assert_eq!(modal.active_field, 3);

    // 2 (Host) down -> 4 (Password)
    modal.active_field = 2;
    modal.nav_down();
    assert_eq!(modal.active_field, 4);

    // 3 (Port) down -> 5 (Alias)
    modal.active_field = 3;
    modal.nav_down();
    assert_eq!(modal.active_field, 5);

    // 4 or 5 down -> 6 (DATA_PATH)
    modal.active_field = 4;
    modal.nav_down();
    assert_eq!(modal.active_field, 6);

    // 6 down -> 7 (Read-Only)
    modal.active_field = 6;
    modal.nav_down();
    assert_eq!(modal.active_field, 7);

    // 7 right -> 8 (Disable SSL)
    modal.nav_right();
    assert_eq!(modal.active_field, 8);
    assert!(modal.disable_ssl);
    modal.toggle_active();
    assert!(!modal.disable_ssl);
    modal.toggle_active();
    assert!(modal.disable_ssl);

    // 7 down -> 9 (Submit)
    modal.active_field = 7;
    modal.nav_down();
    assert_eq!(modal.active_field, 9);
}

#[test]
fn test_new_asset_modal_open_for_group_and_node_categories() {
    let mut modal = NewAssetModalState::new();
    let cases = [
        ("group:tables", 0, ProjectCategory::Table, "Table"),
        ("group:views", 1, ProjectCategory::View, "View"),
        ("group:macros", 2, ProjectCategory::Macro, "Macro"),
        ("group:triggers", 3, ProjectCategory::Trigger, "Trigger"),
        ("group:sequences", 4, ProjectCategory::Sequence, "Sequence"),
        ("group:schemas:empty", 5, ProjectCategory::Schema, "Schema"),
        ("group:queries:empty", 6, ProjectCategory::Query, "Query"),
        ("group:seeds:empty", 7, ProjectCategory::Seed, "Seed"),
    ];

    for (group_name, expected_idx, expected_cat, expected_label) in cases {
        modal.open_for_group_name(group_name);
        assert!(modal.is_open);
        assert_eq!(modal.category_index, expected_idx, "Failed for group {}", group_name);
        assert_eq!(modal.current_category(), expected_cat);
        assert_eq!(modal.category_label(), expected_label);

        let file_node = ExplorerNode {
            name: "test.sql".to_string(),
            display: "test.sql".to_string(),
            level: 1,
            is_expandable: false,
            is_expanded: false,
            node_type: NodeType::ProjectFile {
                path: PathBuf::from("test.sql"),
                category: expected_cat,
            },
        };
        modal.open_for_node(Some(&file_node));
        assert_eq!(modal.category_index, expected_idx);
        assert_eq!(modal.current_category(), expected_cat);
    }
}
