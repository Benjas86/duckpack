use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// DuckDB standard and extended data types.
pub const DUCKDB_DATA_TYPES: &[&str] = &[
    // Integers
    "TINYINT",
    "INT1",
    "SMALLINT",
    "INT2",
    "SHORT",
    "INTEGER",
    "INT",
    "INT4",
    "SIGNED",
    "BIGINT",
    "INT8",
    "LONG",
    "HUGEINT",
    "UTINYINT",
    "USMALLINT",
    "UINTEGER",
    "UINT",
    "UBIGINT",
    "UHUGEINT",
    // Floats & Decimals
    "FLOAT",
    "FLOAT4",
    "REAL",
    "DOUBLE",
    "FLOAT8",
    "PRECISION",
    "DECIMAL",
    "NUMERIC",
    // Boolean
    "BOOLEAN",
    "BOOL",
    // Strings & Binary
    "VARCHAR",
    "CHAR",
    "BPCHAR",
    "TEXT",
    "STRING",
    "BLOB",
    "BYTEA",
    "BINARY",
    "VARBINARY",
    "BIT",
    "BITSTRING",
    // Temporal
    "DATE",
    "TIME",
    "TIMETZ",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TIMESTAMP_S",
    "TIMESTAMP_MS",
    "TIMESTAMP_NS",
    "ZONE",
    "TIMEZONE",
    "INTERVAL",
    // Complex & DuckDB 2.0+ types
    "UUID",
    "JSON",
    "STRUCT",
    "MAP",
    "UNION",
    "LIST",
    "ARRAY",
    "GEOMETRY",
];

/// SQL and DuckDB keywords (DDL, DML, Clauses, Constraints, Control flow).
pub const SQL_KEYWORDS: &[&str] = &[
    // DML & Queries
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "RETURNING",
    "TRUNCATE",
    "MERGE",
    // Clauses & Joins
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "GROUP",
    "ORDER",
    "BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "SAMPLE",
    "QUALIFY",
    "WINDOW",
    "OVER",
    "PARTITION",
    "ROWS",
    "RANGE",
    // Set operations & CTE
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "ALL",
    "DISTINCT",
    "WITH",
    "RECURSIVE",
    "AS",
    // DDL & Object types
    "CREATE",
    "TABLE",
    "SCHEMA",
    "VIEW",
    "INDEX",
    "MACRO",
    "FUNCTION",
    "TYPE",
    "SEQUENCE",
    "TRIGGER",
    "DROP",
    "ALTER",
    "ADD",
    "COLUMN",
    "RENAME",
    "TO",
    "REPLACE",
    "TEMP",
    "TEMPORARY",
    "MATERIALIZED",
    "ATTACH",
    "DETACH",
    "USE",
    "PRAGMA",
    "DESCRIBE",
    "SUMMARIZE",
    "SHOW",
    "EXPLAIN",
    "COPY",
    "EXPORT",
    "IMPORT",
    "CONNECT",
    "DISCONNECT",
    // Constraints & Options
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "DEFAULT",
    "CHECK",
    "UNIQUE",
    "CONSTRAINT",
    "CASCADE",
    "RESTRICT",
    "GENERATED",
    "ALWAYS",
    "IDENTITY",
    "AUTO_INCREMENT",
    "COLLATE",
    "IF",
    "EXISTS",
    // Operators & Conditionals
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "SIMILAR",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "COALESCE",
    "NULLIF",
    "CAST",
    "TRY_CAST",
    // DuckDB special
    "APPROX",
    "VARIANT",
    "POSITIONAL",
    "SECURE",
    "LAMBDA",
];

/// SQL boolean & null literal values.
pub const SQL_LITERALS: &[&str] = &["TRUE", "FALSE", "NULL"];

/// Comprehensive regular expression pattern for highlighting SQL keywords, DuckDB data types,
/// and DDL clauses in the interactive editor (`tui-textarea`).
pub const SQL_KEYWORD_PATTERN: &str = 
    "(?i)\\b(SELECT|FROM|WHERE|INSERT|INTO|VALUES|UPDATE|SET|DELETE|RETURNING|TRUNCATE|MERGE|\
    JOIN|INNER|LEFT|RIGHT|FULL|OUTER|CROSS|NATURAL|ON|USING|GROUP|ORDER|BY|HAVING|LIMIT|\
    OFFSET|SAMPLE|QUALIFY|WINDOW|OVER|PARTITION|ROWS|RANGE|UNION|INTERSECT|EXCEPT|ALL|\
    DISTINCT|WITH|RECURSIVE|AS|CREATE|TABLE|SCHEMA|VIEW|INDEX|MACRO|FUNCTION|TYPE|\
    SEQUENCE|TRIGGER|DROP|ALTER|ADD|COLUMN|RENAME|TO|REPLACE|TEMP|TEMPORARY|MATERIALIZED|\
    ATTACH|DETACH|USE|PRAGMA|DESCRIBE|SUMMARIZE|SHOW|EXPLAIN|COPY|EXPORT|IMPORT|CONNECT|\
    DISCONNECT|PRIMARY|KEY|FOREIGN|REFERENCES|DEFAULT|CHECK|UNIQUE|CONSTRAINT|CASCADE|\
    RESTRICT|GENERATED|ALWAYS|IDENTITY|AUTO_INCREMENT|COLLATE|IF|EXISTS|AND|OR|NOT|IN|IS|\
    BETWEEN|LIKE|ILIKE|SIMILAR|CASE|WHEN|THEN|ELSE|END|COALESCE|NULLIF|CAST|TRY_CAST|\
    APPROX|VARIANT|POSITIONAL|SECURE|LAMBDA|TRUE|FALSE|NULL|\
    TINYINT|INT1|SMALLINT|INT2|SHORT|INTEGER|INT|INT4|SIGNED|BIGINT|INT8|LONG|HUGEINT|\
    UTINYINT|USMALLINT|UINTEGER|UINT|UBIGINT|UHUGEINT|FLOAT|FLOAT4|REAL|DOUBLE|FLOAT8|\
    PRECISION|DECIMAL|NUMERIC|BOOLEAN|BOOL|VARCHAR|CHAR|BPCHAR|TEXT|STRING|BLOB|BYTEA|\
    BINARY|VARBINARY|BIT|BITSTRING|DATE|TIME|TIMETZ|TIMESTAMP|TIMESTAMPTZ|TIMESTAMP_S|\
    TIMESTAMP_MS|TIMESTAMP_NS|ZONE|TIMEZONE|INTERVAL|UUID|JSON|STRUCT|MAP|UNION|LIST|\
    ARRAY|GEOMETRY)\\b";

/// Checks if a given word (case-insensitive) matches any DuckDB data type.
pub fn is_data_type(word: &str) -> bool {
    let upper = word.to_uppercase();
    DUCKDB_DATA_TYPES.iter().any(|&dt| dt == upper)
}

/// Checks if a given word (case-insensitive) matches any SQL keyword or constraint.
pub fn is_keyword(word: &str) -> bool {
    let upper = word.to_uppercase();
    SQL_KEYWORDS.iter().any(|&kw| kw == upper)
}

/// Checks if a given word (case-insensitive) matches a literal (TRUE, FALSE, NULL).
pub fn is_literal(word: &str) -> bool {
    let upper = word.to_uppercase();
    SQL_LITERALS.iter().any(|&lit| lit == upper)
}

/// Tokenizes a SQL string into styled Ratatui `Span`s:
/// - Comments (`-- ...`): DarkGray
/// - Strings (`'...'`): Yellow
/// - Numbers (e.g. `123`, `45.67`): LightGreen
/// - Data types (e.g. `INT`, `DECIMAL`, `VARCHAR`, `TIMESTAMP`): LightMagenta + Bold
/// - Keywords & DDL (e.g. `SELECT`, `CREATE`, `TABLE`, `PRIMARY`, `KEY`, `DEFAULT`): Cyan + Bold
/// - Literals (`TRUE`, `FALSE`, `NULL`): Magenta
/// - Punctuation & other text: default style
pub fn highlight_sql(sql: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // 1. Single-line comment: -- ...
        if chars[i] == '-' && i + 1 < len && chars[i + 1] == '-' {
            let start = i;
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            let comment: String = chars[start..i].iter().collect();
            spans.push(Span::styled(comment, Style::default().fg(Color::DarkGray)));
            continue;
        }

        // 2. String literal: '...' (supporting escaped '' quotes)
        if chars[i] == '\'' {
            let start = i;
            i += 1;
            while i < len {
                if chars[i] == '\'' {
                    if i + 1 < len && chars[i + 1] == '\'' {
                        i += 2; // escaped quote
                    } else {
                        i += 1; // closing quote
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            let str_lit: String = chars[start..i].iter().collect();
            spans.push(Span::styled(str_lit, Style::default().fg(Color::Yellow)));
            continue;
        }

        // 3. Whitespace & Punctuation:
        if chars[i].is_whitespace()
            || chars[i] == '('
            || chars[i] == ')'
            || chars[i] == ','
            || chars[i] == ';'
            || chars[i] == '='
            || chars[i] == '<'
            || chars[i] == '>'
            || chars[i] == '+'
            || chars[i] == '*'
            || chars[i] == '/'
            || chars[i] == '%'
            || chars[i] == '['
            || chars[i] == ']'
            || chars[i] == '.'
            || chars[i] == ':'
        {
            spans.push(Span::raw(chars[i].to_string()));
            i += 1;
            continue;
        }

        // 4. Numeric literal (e.g. 123, 45.67)
        if chars[i].is_ascii_digit() {
            let start = i;
            let mut has_decimal = false;
            while i < len
                && (chars[i].is_ascii_digit()
                    || (chars[i] == '.' && !has_decimal && i + 1 < len && chars[i + 1].is_ascii_digit()))
            {
                if chars[i] == '.' {
                    has_decimal = true;
                }
                i += 1;
            }
            let num: String = chars[start..i].iter().collect();
            spans.push(Span::styled(num, Style::default().fg(Color::LightGreen)));
            continue;
        }

        // 5. Word or Identifier: letters, digits, underscores
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let upper = word.to_uppercase();

            if is_literal(&upper) {
                spans.push(Span::styled(word, Style::default().fg(Color::Magenta)));
            } else if is_data_type(&upper) {
                spans.push(Span::styled(
                    word,
                    Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
                ));
            } else if is_keyword(&upper) {
                spans.push(Span::styled(
                    word,
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw(word));
            }
            continue;
        }

        // Fallback for any other character
        spans.push(Span::raw(chars[i].to_string()));
        i += 1;
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn test_sql_keyword_pattern_matches_data_types() {
        let re = Regex::new(SQL_KEYWORD_PATTERN).unwrap();
        // Scalar types
        assert!(re.is_match("INT"));
        assert!(re.is_match("int"));
        assert!(re.is_match("INTEGER"));
        assert!(re.is_match("integer"));
        assert!(re.is_match("BIGINT"));
        assert!(re.is_match("HUGEINT"));
        assert!(re.is_match("DECIMAL"));
        assert!(re.is_match("numeric"));
        assert!(re.is_match("VARCHAR"));
        assert!(re.is_match("TEXT"));
        assert!(re.is_match("BOOLEAN"));
        assert!(re.is_match("bool"));
        assert!(re.is_match("TIMESTAMP"));
        assert!(re.is_match("timestamptz"));
        assert!(re.is_match("UUID"));
        assert!(re.is_match("JSON"));
        assert!(re.is_match("STRUCT"));
        assert!(re.is_match("MAP"));
        assert!(re.is_match("PRECISION"));

        // DDL and constraints
        assert!(re.is_match("PRIMARY"));
        assert!(re.is_match("KEY"));
        assert!(re.is_match("FOREIGN"));
        assert!(re.is_match("REFERENCES"));
        assert!(re.is_match("DEFAULT"));
        assert!(re.is_match("CASCADE"));
        assert!(re.is_match("UNIQUE"));
        assert!(re.is_match("EXISTS"));
        assert!(re.is_match("RETURNING"));
        assert!(re.is_match("SECURE"));
        assert!(re.is_match("LAMBDA"));
        assert!(re.is_match("VARIANT"));


        // Negative match (regular table or column names)
        assert!(!re.is_match("user_orders"));
        assert!(!re.is_match("customer_id"));
    }

    #[test]
    fn test_highlight_sql_data_types_and_keywords() {
        let sql = "CREATE TABLE finance.q3 (revenue DECIMAL, id INT PRIMARY KEY, active BOOLEAN DEFAULT TRUE);";
        let spans = highlight_sql(sql);

        // Verify CREATE and TABLE are styled Cyan
        let create_span = spans.iter().find(|s| s.content == "CREATE").expect("CREATE span");
        assert_eq!(create_span.style.fg, Some(Color::Cyan));

        let table_span = spans.iter().find(|s| s.content == "TABLE").expect("TABLE span");
        assert_eq!(table_span.style.fg, Some(Color::Cyan));

        // Verify DECIMAL and INT are styled LightMagenta (data types)
        let decimal_span = spans.iter().find(|s| s.content == "DECIMAL").expect("DECIMAL span");
        assert_eq!(decimal_span.style.fg, Some(Color::LightMagenta));

        let int_span = spans.iter().find(|s| s.content == "INT").expect("INT span");
        assert_eq!(int_span.style.fg, Some(Color::LightMagenta));

        // Verify PRIMARY and KEY are styled Cyan
        let primary_span = spans.iter().find(|s| s.content == "PRIMARY").expect("PRIMARY span");
        assert_eq!(primary_span.style.fg, Some(Color::Cyan));

        let key_span = spans.iter().find(|s| s.content == "KEY").expect("KEY span");
        assert_eq!(key_span.style.fg, Some(Color::Cyan));

        // Verify TRUE is styled Magenta (literal)
        let true_span = spans.iter().find(|s| s.content == "TRUE").expect("TRUE span");
        assert_eq!(true_span.style.fg, Some(Color::Magenta));
    }

    #[test]
    fn test_highlight_sql_comments_and_strings() {
        let sql = "-- creating table\nSELECT 'hello world', 42;";
        let spans = highlight_sql(sql);

        let comment_span = spans.iter().find(|s| s.content.starts_with("--")).expect("comment span");
        assert_eq!(comment_span.style.fg, Some(Color::DarkGray));

        let str_span = spans.iter().find(|s| s.content == "'hello world'").expect("string span");
        assert_eq!(str_span.style.fg, Some(Color::Yellow));

        let num_span = spans.iter().find(|s| s.content == "42").expect("number span");
        assert_eq!(num_span.style.fg, Some(Color::LightGreen));
    }
}
