#!/bin/bash
set -e

DEMO_DIR="demo_project"
echo "Scaffolding DuckPack Demo Project in '$DEMO_DIR'..."

rm -rf "$DEMO_DIR"
mkdir -p "$DEMO_DIR/schemas" "$DEMO_DIR/tables" "$DEMO_DIR/views" "$DEMO_DIR/queries" "$DEMO_DIR/macros"

# 1. Environment Profiles
cat << 'EOF' > "$DEMO_DIR/.env.staging"
ENV_PREFIX=staging
TABLE_SUFFIX=dev
EOF

cat << 'EOF' > "$DEMO_DIR/.env.prod"
ENV_PREFIX=prod
TABLE_SUFFIX=live
EOF

# 2. TOML Configuration
cat << 'EOF' > "$DEMO_DIR/duckpack.toml"
[env.staging]
db = "staging.duckdb"

[env.prod]
db = "prod.duckdb"
EOF

# 3. DuckIgnore rules
cat << 'EOF' > "$DEMO_DIR/.duckignore"
tables/ignored_*.sql
queries/secret_*.sql
EOF

cat << 'EOF' > "$DEMO_DIR/tables/ignored_table.sql"
-- This file should be completely ignored by DuckPack because of .duckignore
CREATE TABLE ignored_table (id INT);
EOF

# 3. Core Schemas
cat << 'EOF' > "$DEMO_DIR/schemas/reporting.sql"
CREATE SCHEMA reporting;
EOF

# 4. Core Schema (Tables)
cat << 'EOF' > "$DEMO_DIR/tables/users.sql"
CREATE TABLE ${ENV_PREFIX}_users_${TABLE_SUFFIX} (
    id INT PRIMARY KEY,
    name VARCHAR,
    email VARCHAR,
    created_at TIMESTAMP
);
EOF

cat << 'EOF' > "$DEMO_DIR/tables/orders.sql"
CREATE TABLE ${ENV_PREFIX}_orders_${TABLE_SUFFIX} (
    order_id INT PRIMARY KEY,
    user_id INT,
    total_amount DECIMAL(10, 2),
    order_date DATE,
    FOREIGN KEY (user_id) REFERENCES ${ENV_PREFIX}_users_${TABLE_SUFFIX}(id)
);
EOF

cat << 'EOF' > "$DEMO_DIR/tables/sales.sql"
CREATE TABLE reporting.${ENV_PREFIX}_sales_${TABLE_SUFFIX} (
    sale_id INT PRIMARY KEY,
    order_id INT,
    product_name VARCHAR,
    sale_amount DECIMAL(10, 2),
    sale_date DATE,
    FOREIGN KEY (order_id) REFERENCES ${ENV_PREFIX}_orders_${TABLE_SUFFIX}(order_id)
);
EOF

# 5. Core Schema (Views)
cat << 'EOF' > "$DEMO_DIR/views/user_orders_summary.sql"
CREATE VIEW ${ENV_PREFIX}_user_orders_summary AS
SELECT
    u.name,
    COUNT(o.order_id) as total_orders,
    SUM(o.total_amount) as total_spent
FROM ${ENV_PREFIX}_users_${TABLE_SUFFIX} u
LEFT JOIN ${ENV_PREFIX}_orders_${TABLE_SUFFIX} o ON u.id = o.user_id
GROUP BY u.name;
EOF

# 6. Core Schema (Macros)
cat << 'EOF' > "$DEMO_DIR/macros/calculate_discount.sql"
-- A simple macro to apply a 10% discount
CREATE MACRO calculate_discount(amount) AS amount * 0.90;
EOF

# 7. IDE Queries & Seed Data
cat << 'EOF' > "$DEMO_DIR/queries/01_seed_data.sql"
-- RUN THIS FILE IN THE IDE TO SEED 10,000 ROWS!
-- First, make sure you applied the 'prod' environment profile.

INSERT INTO prod_users_live SELECT range, 'User ' || range, 'user' || range || '@example.com', current_timestamp FROM range(1, 10001);

INSERT INTO prod_orders_live SELECT range, range, (range % 100) * 1.5, current_date FROM range(1, 10001);

INSERT INTO reporting.prod_sales_live SELECT range, range, 'Product ' || (range % 50), (range % 100) * 2.5, current_date FROM range(1, 10001);

SELECT 'Successfully inserted 10,000 rows into users, orders, and sales tables!' as status;
EOF

cat << 'EOF' > "$DEMO_DIR/queries/02_test_syntax_error.sql"
-- This query deliberately contains a syntax error!
-- Highlight it and hit Ctrl+E or F5 to test the Error Highlighter.
SELECT
    id,
    naamee
FRROOOM prod_users_live; 
EOF

cat << 'EOF' > "$DEMO_DIR/queries/03_test_macro.sql"
-- Test querying our user_orders_summary view and using the calculate_discount macro!
SELECT 
    name, 
    total_spent, 
    calculate_discount(total_spent) as discounted_total 
FROM prod_user_orders_summary 
ORDER BY total_spent DESC 
LIMIT 10;
EOF

echo ""
echo "========================================================="
echo "✅ Demo Project Scaffolded Successfully!"
echo "========================================================="
echo "Follow these steps to learn and test the DuckPack framework:"
echo ""
echo "1. Apply the STAGING environment to see what it would build:"
echo "    ./target/release/duckpack apply --project-dir $DEMO_DIR --env staging"
echo "    (Notice how it plans to create tables starting with 'staging_' and connects to 'staging.duckdb' automatically via duckpack.toml)"
echo ""
echo "2. Apply the PROD environment:"
echo "    ./target/release/duckpack apply --project-dir $DEMO_DIR --env prod"
echo "    (Notice how it now plans to create tables starting with 'prod_' and connects to 'prod.duckdb' automatically)"
echo ""
echo "3. Jump into the powerful interactive IDE:"
echo "    ./target/release/duckpack explore --project-dir $DEMO_DIR --env prod"
echo ""
echo "4. While in the IDE:"
echo "   - Press 'Ctrl+V' to test the Schema Visualizer. It will display a beautiful ASCII tree of tables and their foreign-key constraints!"
echo "   - Click on 'queries/01_seed_data.sql' and hit F5 to populate the database with 10,000 rows."
echo "   - Open 'queries/02_test_syntax_error.sql' and execute it to see real-time syntax error highlighting."
echo "   - Open 'queries/03_test_macro.sql' to test your views and macros."
echo "   - Query 'SELECT * FROM prod_users_live;' and scroll to test UI performance and pagination."
echo "========================================================="
