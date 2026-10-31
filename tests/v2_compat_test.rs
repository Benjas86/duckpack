use anyhow::Result;
use duckdb::Connection;
use duckpack::diff::{
    compute_diff, get_macros, get_schema, get_schemas, get_sequences, get_triggers, get_views,
    ConstraintSchema, DiffItemType,
};
use duckpack::engine::{apply_diff, build_shadow_db};
use std::path::PathBuf;


#[test]
fn test_duckdb_v2_compat_end_to_end() -> Result<()> {
    let proj_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v2_0_compat");

    assert!(proj_dir.exists(), "Test fixture directory must exist: {:?}", proj_dir);

    // 1. Build shadow database from declarative project
    let shadow_conn = build_shadow_db(&proj_dir)?;

    // 2. Introspect shadow catalog
    let shadow_schemas = get_schemas(&shadow_conn, None)?;
    assert!(shadow_schemas.contains("core"), "Must discover 'core' schema");
    assert!(shadow_schemas.contains("core.analytics"), "Must discover 'core.analytics' schema");
    assert!(shadow_schemas.contains("core.analytics.reports"), "Must discover 'core.analytics.reports' schema");

    let shadow_tables = get_schema(&shadow_conn, None)?;
    assert!(shadow_tables.contains_key("core.users"), "Must discover 'core.users'");
    assert!(shadow_tables.contains_key("core.analytics.reports.metrics"), "Must discover 'core.analytics.reports.metrics'");
    assert!(shadow_tables.contains_key("core.analytics.audit_log"), "Must discover 'core.analytics.audit_log'");

    // Verify VARIANT data type in shadow
    let users_tbl = shadow_tables.get("core.users").unwrap();
    assert_eq!(
        users_tbl.columns.get("attributes").map(|s| s.as_str()),
        Some("VARIANT"),
        "Attributes column must have VARIANT type in DuckDB 2.0"
    );

    let shadow_views = get_views(&shadow_conn, None)?;
    assert!(shadow_views.contains_key("core.analytics.v_secure_users"), "Must discover secure view");
    assert!(shadow_views.contains_key("core.analytics.reports.v_summary"), "Must discover nested reports view");

    let shadow_macros = get_macros(&shadow_conn, None)?;
    assert!(
        shadow_macros.contains_key("core.analytics.double_all")
            || shadow_macros.contains_key("double_all"),
        "Must discover lambda macro double_all"
    );

    let shadow_triggers = get_triggers(&shadow_conn, None)?;
    assert!(
        shadow_triggers.contains_key("trg_user_audit")
            || shadow_triggers.values().any(|t| t.trigger_name == "trg_user_audit"),
        "Must discover trg_user_audit trigger"
    );

    let shadow_sequences = get_sequences(&shadow_conn, None)?;

    // 3. Compute diff against a fresh, empty target database
    let target_conn = Connection::open_in_memory()?;
    let target_schemas = get_schemas(&target_conn, None)?;
    let target_sequences = get_sequences(&target_conn, None)?;
    let target_tables = get_schema(&target_conn, None)?;
    let target_views = get_views(&target_conn, None)?;
    let target_macros = get_macros(&target_conn, None)?;
    let target_triggers = get_triggers(&target_conn, None)?;

    let diff = compute_diff(
        &shadow_schemas,
        &target_schemas,
        &shadow_sequences,
        &target_sequences,
        &shadow_tables,
        &target_tables,
        &shadow_views,
        &target_views,
        &shadow_macros,
        &target_macros,
        &shadow_triggers,
        &target_triggers,
        &proj_dir,
        &[],
    );

    assert!(!diff.is_empty(), "Initial diff should contain creation items");

    // 4. Apply migrations to target database
    apply_diff(&target_conn, &diff, false, false, &proj_dir, None)?;

    // 5. Test VARIANT column DML and query
    target_conn.execute_batch(
        "INSERT INTO core.users VALUES (101, 'Ada Lovelace', '{\"role\": \"pioneer\", \"level\": 10}'::VARIANT);",
    )?;

    let variant_val: String = target_conn.query_row(
        "SELECT CAST(attributes AS VARCHAR) FROM core.users WHERE id = 101;",
        [],
        |r| r.get(0),
    )?;
    assert!(variant_val.contains("pioneer"), "VARIANT column should store and return JSON structure: {}", variant_val);

    // 6. Test trigger firing on target database
    let audit_count: i64 = target_conn.query_row(
        "SELECT count(*) FROM core.analytics.audit_log WHERE event_id = 101;",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(audit_count, 1, "Trigger trg_user_audit should have inserted a row into audit_log");

    // 7. Test lambda macro execution on target database
    let doubled: String = target_conn.query_row(
        "SELECT CAST(core.analytics.double_all([10, 20, 30]) AS VARCHAR);",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(doubled, "[20, 40, 60]", "Lambda macro should execute correctly: {}", doubled);

    // 8. Test secure view and nested schema views
    let view_user: String = target_conn.query_row(
        "SELECT name FROM core.analytics.v_secure_users WHERE id = 101;",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(view_user, "Ada Lovelace");

    // 9. Test RecreateTable + Trigger Preservation:
    // Suppose shadow modifies core.users to add a CHECK constraint (requiring RecreateTable)
    let mut updated_shadow_tables = shadow_tables.clone();
    let mut users_mod = users_tbl.clone();
    users_mod.constraints.push(ConstraintSchema {
        constraint_type: "CHECK".to_string(),
        constraint_name: Some("chk_user_id".to_string()),
        columns: vec!["id".to_string()],
        expression: Some("id > 0".to_string()),
        referenced_table: None,
        referenced_columns: vec![],
    });
    users_mod.create_sql = "CREATE TABLE core.users (id INT PRIMARY KEY CHECK (id > 0), name VARCHAR NOT NULL, attributes VARIANT);".to_string();
    updated_shadow_tables.insert("core.users".to_string(), users_mod);

    // Current target state after initial apply
    let current_target_schemas = get_schemas(&target_conn, None)?;
    let current_target_sequences = get_sequences(&target_conn, None)?;
    let current_target_tables = get_schema(&target_conn, None)?;
    let current_target_triggers = get_triggers(&target_conn, None)?;

    let mod_diff = compute_diff(
        &shadow_schemas,
        &current_target_schemas,
        &shadow_sequences,
        &current_target_sequences,
        &updated_shadow_tables,
        &current_target_tables,
        &shadow_views,
        &shadow_views,
        &shadow_macros,
        &shadow_macros,
        &shadow_triggers,
        &current_target_triggers,
        &proj_dir,
        &[],
    );

    let has_recreate = mod_diff.items.iter().any(|i| i.item_type == DiffItemType::RecreateTable && i.obj_name == "core.users");
    assert!(has_recreate, "Diff must generate RecreateTable for core.users");

    let has_recreated_trigger = mod_diff.items.iter().any(|i| i.item_type == DiffItemType::CreateTrigger && i.obj_name.contains("trg_user_audit"));
    assert!(has_recreated_trigger, "Diff must automatically re-emit CreateTrigger for trigger on recreated table");

    // Apply the recreation diff
    apply_diff(&target_conn, &mod_diff, true, false, &proj_dir, None)?;

    // Verify existing user data survived copy-and-swap
    let count_after_swap: i64 = target_conn.query_row(
        "SELECT count(*) FROM core.users WHERE id = 101;",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count_after_swap, 1, "User record 101 must survive copy-and-swap migration");

    // Verify trigger still fires after table recreation!
    target_conn.execute_batch(
        "INSERT INTO core.users VALUES (102, 'Charles Babbage', '{\"role\": \"inventor\"}'::VARIANT);",
    )?;
    let audit_count_102: i64 = target_conn.query_row(
        "SELECT count(*) FROM core.analytics.audit_log WHERE event_id = 102;",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(audit_count_102, 1, "Trigger must still be active and fire after table recreation");

    // 10. Test Secure View modification: update view definition
    let mut updated_views = shadow_views.clone();
    updated_views.insert(
        "core.analytics.v_secure_users".to_string(),
        "CREATE SECURE VIEW core.analytics.v_secure_users AS SELECT id, upper(name) AS name FROM core.users;".to_string(),
    );

    let view_diff = compute_diff(
        &shadow_schemas,
        &current_target_schemas,
        &shadow_sequences,
        &current_target_sequences,
        &updated_shadow_tables,
        &updated_shadow_tables,
        &updated_views,
        &shadow_views,
        &shadow_macros,
        &shadow_macros,
        &shadow_triggers,
        &shadow_triggers,
        &proj_dir,
        &[],
    );

    let secure_view_item = view_diff.items.iter().find(|i| i.obj_name == "core.analytics.v_secure_users");
    assert!(secure_view_item.is_some(), "Must generate diff item for modified secure view");
    assert!(
        secure_view_item.unwrap().sql.contains("CREATE OR REPLACE SECURE VIEW"),
        "SQL must use CREATE OR REPLACE SECURE VIEW: {}",
        secure_view_item.unwrap().sql
    );

    apply_diff(&target_conn, &view_diff, false, false, &proj_dir, None)?;

    let upper_name: String = target_conn.query_row(
        "SELECT name FROM core.analytics.v_secure_users WHERE id = 101;",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(upper_name, "ADA LOVELACE", "Updated secure view should return upper-case name");

    Ok(())
}
