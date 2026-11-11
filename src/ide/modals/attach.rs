use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use std::path::Path;

use super::input::{
    delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
    render_text_with_cursor,
};

/// Supported database presets for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AttachProvider {
    DuckDB,
    Quack,
    MotherDuck,
    SQLite,
    PostgreSQL,
    MySQL,
    MSSQL,
    DuckLake,
}

impl AttachProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            AttachProvider::DuckDB => "DuckDB (Local / In-Memory)",
            AttachProvider::Quack => "Remote Quack Server",
            AttachProvider::MotherDuck => "MotherDuck Cloud",
            AttachProvider::SQLite => "SQLite",
            AttachProvider::PostgreSQL => "PostgreSQL",
            AttachProvider::MySQL => "MySQL",
            AttachProvider::MSSQL => "SQL Server / MSSQL (Community)",
            AttachProvider::DuckLake => "DuckLake (Lakehouse / S3)",
        }
    }

    pub(crate) fn is_relational(&self) -> bool {
        matches!(
            self,
            AttachProvider::PostgreSQL | AttachProvider::MySQL | AttachProvider::MSSQL
        )
    }

    pub(crate) fn is_ducklake(&self) -> bool {
        matches!(self, AttachProvider::DuckLake)
    }

    pub(crate) fn is_quack(&self) -> bool {
        matches!(self, AttachProvider::Quack)
    }

    pub(crate) fn is_motherduck(&self) -> bool {
        matches!(self, AttachProvider::MotherDuck)
    }

    #[allow(dead_code)]
    pub(crate) fn is_file_based(&self) -> bool {
        matches!(self, AttachProvider::DuckDB | AttachProvider::SQLite)
    }
}

/// Supported metadata catalog backends for DuckLake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DuckLakeBackend {
    Quack,
    PostgreSQL,
    LocalFile,
    SQLite,
}

impl DuckLakeBackend {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            DuckLakeBackend::Quack => "Remote Quack Server",
            DuckLakeBackend::PostgreSQL => "PostgreSQL Catalog",
            DuckLakeBackend::LocalFile => "Local File (.ducklake / DuckDB)",
            DuckLakeBackend::SQLite => "SQLite Catalog (.db)",
        }
    }

    pub(crate) fn next(&self) -> Self {
        match self {
            DuckLakeBackend::Quack => DuckLakeBackend::PostgreSQL,
            DuckLakeBackend::PostgreSQL => DuckLakeBackend::LocalFile,
            DuckLakeBackend::LocalFile => DuckLakeBackend::SQLite,
            DuckLakeBackend::SQLite => DuckLakeBackend::Quack,
        }
    }

    pub(crate) fn prev(&self) -> Self {
        match self {
            DuckLakeBackend::Quack => DuckLakeBackend::SQLite,
            DuckLakeBackend::PostgreSQL => DuckLakeBackend::Quack,
            DuckLakeBackend::LocalFile => DuckLakeBackend::PostgreSQL,
            DuckLakeBackend::SQLite => DuckLakeBackend::LocalFile,
        }
    }
}

/// State for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Debug)]
pub(crate) struct AttachModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: DuckDB, 1: Quack, 2: MotherDuck, 3: SQLite, 4: PostgreSQL, 5: MySQL, 6: MSSQL, 7: DuckLake
    pub(crate) ducklake_backend: DuckLakeBackend, // Quack, PostgreSQL, LocalFile, SQLite
    pub(crate) path: String,          // For DuckDB / SQLite / DuckLake file path, MotherDuck DB name
    pub(crate) host: String,          // Host / Server (localhost, etc.)
    pub(crate) port: String,          // Port (1433, 5432, 3306, 9494)
    pub(crate) database: String,      // Database name (mydb, postgres, etc.)
    pub(crate) username: String,      // Username / User ID (sa, postgres, root)
    pub(crate) password: String,      // Password / Auth Token (Masked)
    pub(crate) alias: String,         // Catalog alias
    pub(crate) alias_is_custom: bool,
    pub(crate) data_path: String,     // DuckLake Parquet / storage DATA_PATH (e.g. s3://bucket/data/)
    pub(crate) cursor_pos: usize,
    pub(crate) read_only: bool,
    pub(crate) disable_ssl: bool,
    pub(crate) insecure: bool,
    pub(crate) trusted_connection: bool,
    pub(crate) active_field: usize,
    pub(crate) error_msg: Option<String>,
}

impl AttachModalState {
    pub(crate) fn new() -> Self {
        let mut s = Self {
            is_open: false,
            provider_index: 0,
            ducklake_backend: DuckLakeBackend::Quack,
            path: String::new(),
            host: String::new(),
            port: String::new(),
            database: String::new(),
            username: String::new(),
            password: String::new(),
            alias: String::new(),
            alias_is_custom: false,
            data_path: String::new(),
            cursor_pos: 0,
            read_only: false,
            disable_ssl: false,
            insecure: false,
            trusted_connection: false,
            active_field: 1,
            error_msg: None,
        };
        s.set_provider(0);
        s
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.ducklake_backend = DuckLakeBackend::Quack;
        self.path.clear();
        self.host.clear();
        self.port.clear();
        self.database.clear();
        self.username.clear();
        self.password.clear();
        self.alias.clear();
        self.alias_is_custom = false;
        self.data_path.clear();
        self.cursor_pos = 0;
        self.read_only = false;
        self.disable_ssl = false;
        self.insecure = false;
        self.trusted_connection = false;
        self.active_field = 1;
        self.error_msg = None;
        self.set_provider(0);
    }

    pub(crate) fn provider(&self) -> AttachProvider {
        match self.provider_index {
            1 => AttachProvider::Quack,
            2 => AttachProvider::MotherDuck,
            3 => AttachProvider::SQLite,
            4 => AttachProvider::PostgreSQL,
            5 => AttachProvider::MySQL,
            6 => AttachProvider::MSSQL,
            7 => AttachProvider::DuckLake,
            _ => AttachProvider::DuckDB,
        }
    }

    pub(crate) fn set_ducklake_backend(&mut self, backend: DuckLakeBackend) {
        self.ducklake_backend = backend;
        self.error_msg = None;
        let is_default_port = matches!(self.port.as_str(), "" | "1433" | "5432" | "3306" | "9494");
        let is_default_user = matches!(self.username.as_str(), "" | "sa" | "postgres" | "root" | "lakehouse");
        let is_default_host = matches!(self.host.as_str(), "" | "localhost");
        let is_default_path = matches!(self.path.as_str(), "" | "metadata.ducklake" | "metadata.db" | "data.duckdb");

        match backend {
            DuckLakeBackend::Quack => {
                if is_default_host {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "9494".to_string();
                }
                if self.host == "localhost" || self.host == "127.0.0.1" || self.host == "0.0.0.0" {
                    self.disable_ssl = true;
                }
            }
            DuckLakeBackend::PostgreSQL => {
                if is_default_host {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "5432".to_string();
                }
                if is_default_user {
                    self.username = "lakehouse".to_string();
                }
                if self.database.is_empty() {
                    self.database = "ducklake".to_string();
                }
            }
            DuckLakeBackend::LocalFile => {
                if is_default_path {
                    self.path = "metadata.ducklake".to_string();
                }
            }
            DuckLakeBackend::SQLite => {
                if is_default_path {
                    self.path = "metadata.db".to_string();
                }
            }
        }
        if self.data_path.is_empty() {
            self.data_path = "s3://ducklake/data/".to_string();
        }
        if !self.alias_is_custom {
            self.alias = "my_lakehouse".to_string();
        }
        let total = self.field_count();
        if self.active_field >= total {
            self.active_field = 1;
        }
        self.sync_cursor_to_end();
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 8;
        self.error_msg = None;
        let is_default_port = matches!(self.port.as_str(), "" | "1433" | "5432" | "3306" | "9494");
        let is_default_user = matches!(self.username.as_str(), "" | "sa" | "postgres" | "root" | "lakehouse");

        match self.provider() {
            AttachProvider::DuckLake => {
                self.set_ducklake_backend(self.ducklake_backend);
            }
            AttachProvider::MSSQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "1433".to_string();
                }
                if cfg!(target_os = "windows") {
                    self.trusted_connection = true;
                    self.username.clear();
                } else {
                    self.trusted_connection = false;
                    if is_default_user {
                        self.username = "sa".to_string();
                    }
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "mssql_db".to_string()
                    };
                }
            }
            AttachProvider::PostgreSQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "5432".to_string();
                }
                if is_default_user {
                    self.username = "postgres".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "pg_db".to_string()
                    };
                }
            }
            AttachProvider::MySQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "3306".to_string();
                }
                if is_default_user {
                    self.username = "root".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "mysql_db".to_string()
                    };
                }
            }
            AttachProvider::Quack => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "9494".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = "quack_remote".to_string();
                }
            }
            AttachProvider::MotherDuck => {
                if !self.alias_is_custom {
                    self.alias = if !self.path.is_empty() {
                        self.path.trim_start_matches("md:").to_string()
                    } else {
                        "my_db".to_string()
                    };
                }
            }
            AttachProvider::SQLite => {
                if !self.alias_is_custom {
                    self.alias = "sqlite_db".to_string();
                }
            }
            AttachProvider::DuckDB => {
                if !self.alias_is_custom {
                    self.alias = "attached_db".to_string();
                }
            }
        }
        let total = self.field_count();
        if self.active_field >= total {
            self.active_field = 1;
        }
        self.sync_cursor_to_end();
    }

    pub(crate) fn field_count(&self) -> usize {
        let p = self.provider();
        if p == AttachProvider::MSSQL {
            13
        } else if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => 11,
                DuckLakeBackend::PostgreSQL => 12,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 8,
            }
        } else if p.is_relational() {
            12
        } else if p.is_quack() {
            10
        } else if p.is_motherduck() {
            7
        } else {
            6
        }
    }

    pub(crate) fn submit_index(&self) -> usize {
        let p = self.provider();
        if p == AttachProvider::MSSQL {
            11
        } else if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => 9,
                DuckLakeBackend::PostgreSQL => 10,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 6,
            }
        } else if p.is_relational() {
            10
        } else if p.is_quack() {
            8
        } else if p.is_motherduck() {
            5
        } else {
            4
        }
    }

    pub(crate) fn cancel_index(&self) -> usize {
        let p = self.provider();
        if p == AttachProvider::MSSQL {
            12
        } else if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => 10,
                DuckLakeBackend::PostgreSQL => 11,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 7,
            }
        } else if p.is_relational() {
            11
        } else if p.is_quack() {
            9
        } else if p.is_motherduck() {
            6
        } else {
            5
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_submit(&self) -> bool {
        self.active_field == self.submit_index()
    }

    pub(crate) fn is_cancel(&self) -> bool {
        self.active_field == self.cancel_index()
    }

    pub(crate) fn is_toggle_field(&self) -> bool {
        let p = self.provider();
        if p == AttachProvider::MSSQL {
            (7..=10).contains(&self.active_field)
        } else if p == AttachProvider::DuckLake {
            if self.active_field == 1 {
                true
            } else {
                match self.ducklake_backend {
                    DuckLakeBackend::Quack => matches!(self.active_field, 7 | 8),
                    DuckLakeBackend::PostgreSQL => self.active_field == 9,
                    DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => self.active_field == 5,
                }
            }
        } else if p.is_relational() {
            (7..=9).contains(&self.active_field)
        } else if p.is_quack() {
            (5..=7).contains(&self.active_field)
        } else if p.is_motherduck() {
            self.active_field == 4
        } else {
            self.active_field == 3
        }
    }

    pub(crate) fn is_text_field(&self) -> bool {
        if self.active_field == 0 {
            return false;
        }
        if self.provider() == AttachProvider::DuckLake && self.active_field == 1 {
            return false;
        }
        if self.is_toggle_field() || self.is_submit() || self.is_cancel() {
            return false;
        }
        true
    }

    pub(crate) fn active_text(&self) -> Option<&str> {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => match self.active_field {
                    2 => Some(&self.host),
                    3 => Some(&self.port),
                    4 => Some(&self.password),
                    5 => Some(&self.alias),
                    6 => Some(&self.data_path),
                    _ => None,
                },
                DuckLakeBackend::PostgreSQL => match self.active_field {
                    2 => Some(&self.host),
                    3 => Some(&self.port),
                    4 => Some(&self.database),
                    5 => Some(&self.username),
                    6 => Some(&self.password),
                    7 => Some(&self.alias),
                    8 => Some(&self.data_path),
                    _ => None,
                },
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => match self.active_field {
                    2 => Some(&self.path),
                    3 => Some(&self.alias),
                    4 => Some(&self.data_path),
                    _ => None,
                },
            }
        } else if p.is_relational() {
            match self.active_field {
                1 => Some(&self.host),
                2 => Some(&self.port),
                3 => Some(&self.database),
                4 => Some(&self.username),
                5 => Some(&self.password),
                6 => Some(&self.alias),
                _ => None,
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => Some(&self.host),
                2 => Some(&self.port),
                3 => Some(&self.password),
                4 => Some(&self.alias),
                _ => None,
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => Some(&self.path),
                2 => Some(&self.password),
                3 => Some(&self.alias),
                _ => None,
            }
        } else {
            match self.active_field {
                1 => Some(&self.path),
                2 => Some(&self.alias),
                _ => None,
            }
        }
    }

    pub(crate) fn active_text_mut(&mut self) -> Option<&mut String> {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => match self.active_field {
                    2 => Some(&mut self.host),
                    3 => Some(&mut self.port),
                    4 => Some(&mut self.password),
                    5 => Some(&mut self.alias),
                    6 => Some(&mut self.data_path),
                    _ => None,
                },
                DuckLakeBackend::PostgreSQL => match self.active_field {
                    2 => Some(&mut self.host),
                    3 => Some(&mut self.port),
                    4 => Some(&mut self.database),
                    5 => Some(&mut self.username),
                    6 => Some(&mut self.password),
                    7 => Some(&mut self.alias),
                    8 => Some(&mut self.data_path),
                    _ => None,
                },
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => match self.active_field {
                    2 => Some(&mut self.path),
                    3 => Some(&mut self.alias),
                    4 => Some(&mut self.data_path),
                    _ => None,
                },
            }
        } else if p.is_relational() {
            match self.active_field {
                1 => Some(&mut self.host),
                2 => Some(&mut self.port),
                3 => Some(&mut self.database),
                4 => Some(&mut self.username),
                5 => Some(&mut self.password),
                6 => Some(&mut self.alias),
                _ => None,
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => Some(&mut self.host),
                2 => Some(&mut self.port),
                3 => Some(&mut self.password),
                4 => Some(&mut self.alias),
                _ => None,
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => Some(&mut self.path),
                2 => Some(&mut self.password),
                3 => Some(&mut self.alias),
                _ => None,
            }
        } else {
            match self.active_field {
                1 => Some(&mut self.path),
                2 => Some(&mut self.alias),
                _ => None,
            }
        }
    }

    pub(crate) fn sync_cursor_to_end(&mut self) {
        self.cursor_pos = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
    }

    pub(crate) fn is_port_field(&self) -> bool {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            matches!(self.ducklake_backend, DuckLakeBackend::Quack | DuckLakeBackend::PostgreSQL)
                && self.active_field == 3
        } else if p.is_relational() || p.is_quack() {
            self.active_field == 2
        } else {
            false
        }
    }

    pub(crate) fn is_alias_field(&self) -> bool {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            match self.ducklake_backend {
                DuckLakeBackend::Quack => self.active_field == 5,
                DuckLakeBackend::PostgreSQL => self.active_field == 7,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => self.active_field == 3,
            }
        } else if p.is_relational() {
            self.active_field == 6
        } else if p.is_quack() {
            self.active_field == 4
        } else if p.is_motherduck() {
            self.active_field == 3
        } else {
            self.active_field == 2
        }
    }

    pub(crate) fn sync_alias_if_default(&mut self) {
        if self.alias_is_custom {
            return;
        }
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            if matches!(self.ducklake_backend, DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite) {
                let stem = Path::new(&self.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if !stem.is_empty() {
                    self.alias = stem.to_string();
                }
            }
        } else if p.is_relational() {
            if !self.database.is_empty() {
                self.alias = self.database.clone();
            }
        } else if p.is_motherduck() {
            let clean = self.path.trim_start_matches("md:");
            if !clean.is_empty() {
                self.alias = clean.to_string();
            }
        } else if matches!(p, AttachProvider::DuckDB | AttachProvider::SQLite) {
            let stem = Path::new(&self.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if !stem.is_empty() {
                self.alias = stem.to_string();
            }
        }
    }

    pub(crate) fn toggle_active(&mut self) {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            if self.active_field == 1 {
                self.set_ducklake_backend(self.ducklake_backend.next());
            } else {
                match self.ducklake_backend {
                    DuckLakeBackend::Quack => match self.active_field {
                        7 => self.read_only = !self.read_only,
                        8 => self.disable_ssl = !self.disable_ssl,
                        _ => {}
                    },
                    DuckLakeBackend::PostgreSQL => {
                        if self.active_field == 9 {
                            self.read_only = !self.read_only;
                        }
                    }
                    DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => {
                        if self.active_field == 5 {
                            self.read_only = !self.read_only;
                        }
                    }
                }
            }
        } else if p == AttachProvider::MSSQL {
            match self.active_field {
                7 => self.read_only = !self.read_only,
                8 => {
                    self.trusted_connection = !self.trusted_connection;
                    if self.trusted_connection {
                        if self.username == "sa" {
                            self.username.clear();
                        }
                    } else if self.username.is_empty() {
                        self.username = "sa".to_string();
                    }
                }
                9 => self.disable_ssl = !self.disable_ssl,
                10 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_relational() {
            match self.active_field {
                7 => self.read_only = !self.read_only,
                8 => self.disable_ssl = !self.disable_ssl,
                9 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                5 => self.read_only = !self.read_only,
                6 => self.disable_ssl = !self.disable_ssl,
                7 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_motherduck() {
            if self.active_field == 4 {
                self.read_only = !self.read_only;
            }
        } else if self.active_field == 3 {
            self.read_only = !self.read_only;
        }
    }

    pub(crate) fn next_field(&mut self) {
        let total = self.field_count();
        self.active_field = (self.active_field + 1) % total;
        self.sync_cursor_to_end();
    }

    pub(crate) fn prev_field(&mut self) {
        let total = self.field_count();
        self.active_field = if self.active_field == 0 {
            total - 1
        } else {
            self.active_field - 1
        };
        self.sync_cursor_to_end();
    }

    pub(crate) fn nav_left(&mut self) {
        if self.active_field == 0 {
            let prev = if self.provider_index == 0 {
                7
            } else {
                self.provider_index - 1
            };
            self.set_provider(prev);
        } else if self.provider() == AttachProvider::DuckLake && self.active_field == 1 {
            self.set_ducklake_backend(self.ducklake_backend.prev());
        } else if self.provider() == AttachProvider::DuckLake
            && self.ducklake_backend == DuckLakeBackend::Quack
            && matches!(self.active_field, 7 | 8)
        {
            self.active_field = if self.active_field == 7 { 8 } else { 7 };
        } else if self.is_cancel() {
            self.active_field = self.submit_index();
        } else if self.is_submit() {
            self.active_field = self.cancel_index();
        } else if self.is_toggle_field() {
            self.toggle_active();
        }
    }

    pub(crate) fn nav_right(&mut self) {
        if self.active_field == 0 {
            let next = (self.provider_index + 1) % 8;
            self.set_provider(next);
        } else if self.provider() == AttachProvider::DuckLake && self.active_field == 1 {
            self.set_ducklake_backend(self.ducklake_backend.next());
        } else if self.provider() == AttachProvider::DuckLake
            && self.ducklake_backend == DuckLakeBackend::Quack
            && matches!(self.active_field, 7 | 8)
        {
            self.active_field = if self.active_field == 7 { 8 } else { 7 };
        } else if self.is_submit() {
            self.active_field = self.cancel_index();
        } else if self.is_cancel() {
            self.active_field = self.submit_index();
        } else if self.is_toggle_field() {
            self.toggle_active();
        }
    }

    pub(crate) fn nav_down(&mut self) {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            self.active_field = match self.ducklake_backend {
                DuckLakeBackend::Quack => match self.active_field {
                    0 => 2,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 | 5 => 6,
                    6 => 7,
                    7 => 9,
                    8 => 10,
                    9 | 10 => 0,
                    _ => 0,
                },
                DuckLakeBackend::PostgreSQL => match self.active_field {
                    0 => 2,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 6,
                    5 => 7,
                    6 | 7 => 8,
                    8 => 9,
                    9 => 10,
                    10 | 11 => 0,
                    _ => 0,
                },
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => match self.active_field {
                    0 => 2,
                    1 => 3,
                    2 | 3 => 4,
                    4 => 5,
                    5 => 6,
                    6 | 7 => 0,
                    _ => 0,
                },
            };
        } else if p == AttachProvider::MSSQL {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5 | 6 => 7,
                7..=10 => 11,
                11 | 12 => 0,
                _ => 0,
            };
        } else if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5 | 6 => 7,
                7..=9 => 10,
                10 | 11 => 0,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5..=7 => 8,
                8 | 9 => 0,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 | 3 => 4,
                4 => 5,
                5 | 6 => 0,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 => 3,
                3 => 4,
                4 | 5 => 0,
                _ => 0,
            };
        }
        self.sync_cursor_to_end();
    }

    pub(crate) fn nav_up(&mut self) {
        let p = self.provider();
        if p == AttachProvider::DuckLake {
            self.active_field = match self.ducklake_backend {
                DuckLakeBackend::Quack => match self.active_field {
                    0 => 9,
                    1 => 10,
                    2 => 0,
                    3 => 1,
                    4 => 2,
                    5 => 3,
                    6 => 4,
                    7 | 8 => 6,
                    9 => 7,
                    10 => 8,
                    _ => 0,
                },
                DuckLakeBackend::PostgreSQL => match self.active_field {
                    0 => 10,
                    1 => 11,
                    2 => 0,
                    3 => 1,
                    4 => 2,
                    5 => 3,
                    6 => 4,
                    7 => 5,
                    8 => 6,
                    9 => 8,
                    10 | 11 => 9,
                    _ => 0,
                },
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => match self.active_field {
                    0 => 6,
                    1 => 7,
                    2 => 0,
                    3 => 1,
                    4 => 2,
                    5 => 4,
                    6 | 7 => 5,
                    _ => 0,
                },
            };
        } else if p == AttachProvider::MSSQL {
            self.active_field = match self.active_field {
                0 => 11,
                1 | 2 => 0,
                3 | 4 => 1,
                5 | 6 => 3,
                7..=10 => 5,
                11 | 12 => 7,
                _ => 0,
            };
        } else if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 10,
                1 | 2 => 0,
                3 | 4 => 1,
                5 | 6 => 3,
                7..=9 => 5,
                10 | 11 => 7,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 8,
                1 | 2 => 0,
                3 | 4 => 1,
                5..=7 => 3,
                8 | 9 => 5,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 5,
                1 => 0,
                2 | 3 => 1,
                4 => 2,
                5 | 6 => 4,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 4,
                1 => 0,
                2 => 1,
                3 => 2,
                4 | 5 => 3,
                _ => 0,
            };
        }
        self.sync_cursor_to_end();
    }

    pub(crate) fn handle_char(&mut self, c: char) {
        if self.is_port_field() && !c.is_ascii_digit() {
            return;
        }
        if self.is_alias_field() {
            self.alias_is_custom = true;
        }
        let mut cur = self.cursor_pos;
        if let Some(t) = self.active_text_mut() {
            insert_char_at_cursor(t, &mut cur, c);
        }
        self.cursor_pos = cur;
        self.sync_alias_if_default();
    }

    pub(crate) fn handle_backspace(&mut self) {
        if self.is_alias_field() {
            self.alias_is_custom = true;
        }
        let mut cur = self.cursor_pos;
        if let Some(t) = self.active_text_mut() {
            delete_char_before_cursor(t, &mut cur);
        }
        self.cursor_pos = cur;
        self.sync_alias_if_default();
    }

    pub(crate) fn handle_delete(&mut self) {
        if self.is_alias_field() {
            self.alias_is_custom = true;
        }
        let cur = self.cursor_pos;
        if let Some(t) = self.active_text_mut() {
            delete_char_at_cursor(t, cur);
        }
        self.sync_alias_if_default();
    }

    pub(crate) fn build_connection_target(&self) -> std::result::Result<(String, String), String> {
        let provider = self.provider();
        match provider {
            AttachProvider::DuckLake => {
                let backend_str = match self.ducklake_backend {
                    DuckLakeBackend::Quack => "quack",
                    DuckLakeBackend::PostgreSQL => "postgres",
                    DuckLakeBackend::LocalFile => "file",
                    DuckLakeBackend::SQLite => "sqlite",
                };
                let target = crate::engine::format_ducklake_target(
                    backend_str,
                    &self.host,
                    &self.port,
                    &self.database,
                    &self.username,
                    &self.password,
                    &self.path,
                )?;
                let alias = if self.alias.trim().is_empty() {
                    "my_lakehouse".to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::MSSQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MSSQL.".to_string());
                }
                let conn_str = crate::engine::format_mssql_connection_string(
                    &self.host,
                    &self.port,
                    &self.database,
                    &self.username,
                    &self.password,
                    self.trusted_connection,
                    self.insecure,
                    self.disable_ssl,
                );
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "mssql_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((conn_str.replace('\'', "''"), alias))
            }
            AttachProvider::PostgreSQL => {
                if self.host.trim().is_empty() {
                    return Err(
                        "Host / Server address cannot be empty for PostgreSQL.".to_string()
                    );
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "5432"
                    } else {
                        self.port.trim()
                    };
                    let user_part =
                        if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                            format!("{}:{}@", self.username.trim(), self.password.trim())
                        } else if !self.username.trim().is_empty() {
                            format!("{}@", self.username.trim())
                        } else {
                            String::new()
                        };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("postgres://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") {
                            uri.push_str("&sslmode=disable");
                        }
                    } else {
                        uri.push_str("?sslmode=disable");
                    }
                } else if self.insecure {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") {
                            uri.push_str("&sslmode=prefer");
                        }
                    } else {
                        uri.push_str("?sslmode=prefer");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "pg_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::MySQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MySQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "3306"
                    } else {
                        self.port.trim()
                    };
                    let user_part =
                        if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                            format!("{}:{}@", self.username.trim(), self.password.trim())
                        } else if !self.username.trim().is_empty() {
                            format!("{}@", self.username.trim())
                        } else {
                            String::new()
                        };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("mysql://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("ssl-mode=") {
                            uri.push_str("&ssl-mode=DISABLED");
                        }
                    } else {
                        uri.push_str("?ssl-mode=DISABLED");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "mysql_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::Quack => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server cannot be empty for Remote Quack.".to_string());
                }
                let target = crate::engine::format_quack_endpoint(&self.host, &self.port);
                let alias = if self.alias.trim().is_empty() {
                    "quack_remote".to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::MotherDuck => {
                if self.path.trim().is_empty() {
                    return Err("MotherDuck database name cannot be empty.".to_string());
                }
                let target = if self.path.trim().starts_with("md:") {
                    self.path.trim().to_string()
                } else {
                    format!("md:{}", self.path.trim())
                };
                let alias = if self.alias.trim().is_empty() {
                    self.path.trim().trim_start_matches("md:").to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::SQLite | AttachProvider::DuckDB => {
                if self.path.trim().is_empty() {
                    return Err("Database file path cannot be empty.".to_string());
                }
                let alias = if self.alias.trim().is_empty() {
                    Path::new(self.path.trim())
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("attached_db")
                        .to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((self.path.trim().replace('\'', "''"), alias))
            }
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 82.min(f.area().width.saturating_sub(4));
        let err_lines = if self.error_msg.is_some() { 3 } else { 1 };
        let is_relational = self.provider().is_relational();
        let is_ducklake = self.provider().is_ducklake();
        let popup_height = if is_ducklake {
            let base = match self.ducklake_backend {
                DuckLakeBackend::Quack => 17,
                DuckLakeBackend::PostgreSQL => 20,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 14,
            };
            (base + err_lines + 2).min(f.area().height.saturating_sub(2))
        } else if is_relational {
            (17 + err_lines + 2).min(f.area().height.saturating_sub(2))
        } else {
            (14 + err_lines + 2).min(f.area().height.saturating_sub(2))
        };
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🔌 Attach Database (Ctrl+A) ")
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);

        if is_ducklake {
            let (field_layout, submit_idx, cancel_idx, ro_idx, data_path_idx) = match self.ducklake_backend {
                DuckLakeBackend::Quack => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),         // 0: Provider Preset & Backend Picker
                            Constraint::Length(3),         // 1: Host & Port
                            Constraint::Length(3),         // 2: Password / Token & Alias
                            Constraint::Length(3),         // 3: Storage DATA_PATH
                            Constraint::Length(2),         // 4: Read-Only, Disable SSL toggles & S3 tip
                            Constraint::Length(err_lines), // 5: Error/hint
                            Constraint::Length(3),         // 6: Buttons
                        ])
                        .split(inner_area);
                    (layout, 9, 10, 7, 6)
                }
                DuckLakeBackend::PostgreSQL => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),         // 0: Provider Preset & Backend Picker
                            Constraint::Length(3),         // 1: Host & Port
                            Constraint::Length(3),         // 2: Database & Username
                            Constraint::Length(3),         // 3: Password & Alias
                            Constraint::Length(3),         // 4: Storage DATA_PATH
                            Constraint::Length(2),         // 5: Read-Only toggle & S3 tip
                            Constraint::Length(err_lines), // 6: Error/hint
                            Constraint::Length(3),         // 7: Buttons
                        ])
                        .split(inner_area);
                    (layout, 10, 11, 9, 8)
                }
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => {
                    let layout = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),         // 0: Provider Preset & Backend Picker
                            Constraint::Length(3),         // 1: File Path & Alias
                            Constraint::Length(3),         // 2: Storage DATA_PATH
                            Constraint::Length(2),         // 3: Read-Only toggle & S3 tip
                            Constraint::Length(err_lines), // 4: Error/hint
                            Constraint::Length(3),         // 5: Buttons
                        ])
                        .split(inner_area);
                    (layout, 6, 7, 5, 4)
                }
            };

            // 0: Row 0 split into Provider Preset (Left) and DuckLake Backend (Right)
            let top_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[0]);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Provider (◄ / ►) ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, top_cols[0]);

            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let backend_label = format!("  ◄ [ {} ] ►", self.ducklake_backend.name());
            let p1 = Paragraph::new(backend_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Backend (◄ / ► / Space) ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, top_cols[1]);

            // Backend specific rows
            match self.ducklake_backend {
                DuckLakeBackend::Quack => {
                    let host_port_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                        .split(field_layout[1]);
                    let f2_style = if self.active_field == 2 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p2_line = render_text_with_cursor(
                        &self.host,
                        self.cursor_pos,
                        self.active_field == 2,
                        false,
                    );
                    let p2 = Paragraph::new(p2_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Quack Catalog Host / Server (e.g. localhost) ")
                            .border_style(f2_style),
                    );
                    f.render_widget(p2, host_port_cols[0]);

                    let f3_style = if self.active_field == 3 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p3_line = render_text_with_cursor(
                        &self.port,
                        self.cursor_pos,
                        self.active_field == 3,
                        false,
                    );
                    let p3 = Paragraph::new(p3_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Port (9494) ")
                            .border_style(f3_style),
                    );
                    f.render_widget(p3, host_port_cols[1]);

                    let pass_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[2]);
                    let f4_style = if self.active_field == 4 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p4_line = render_text_with_cursor(
                        &self.password,
                        self.cursor_pos,
                        self.active_field == 4,
                        true,
                    );
                    let p4 = Paragraph::new(p4_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Quack Auth Token / Password (Masked) ")
                            .border_style(f4_style),
                    );
                    f.render_widget(p4, pass_alias_cols[0]);

                    let f5_style = if self.active_field == 5 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p5_line = render_text_with_cursor(
                        &self.alias,
                        self.cursor_pos,
                        self.active_field == 5,
                        false,
                    );
                    let p5 = Paragraph::new(p5_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Lakehouse Catalog Alias ")
                            .border_style(f5_style),
                    );
                    f.render_widget(p5, pass_alias_cols[1]);
                }
                DuckLakeBackend::PostgreSQL => {
                    let host_port_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                        .split(field_layout[1]);
                    let f2_style = if self.active_field == 2 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p2_line = render_text_with_cursor(
                        &self.host,
                        self.cursor_pos,
                        self.active_field == 2,
                        false,
                    );
                    let p2 = Paragraph::new(p2_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" PostgreSQL Catalog Host (e.g. localhost) ")
                            .border_style(f2_style),
                    );
                    f.render_widget(p2, host_port_cols[0]);

                    let f3_style = if self.active_field == 3 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p3_line = render_text_with_cursor(
                        &self.port,
                        self.cursor_pos,
                        self.active_field == 3,
                        false,
                    );
                    let p3 = Paragraph::new(p3_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Port (5432) ")
                            .border_style(f3_style),
                    );
                    f.render_widget(p3, host_port_cols[1]);

                    let db_user_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[2]);
                    let f4_style = if self.active_field == 4 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p4_line = render_text_with_cursor(
                        &self.database,
                        self.cursor_pos,
                        self.active_field == 4,
                        false,
                    );
                    let p4 = Paragraph::new(p4_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Metadata Database Name (e.g. ducklake) ")
                            .border_style(f4_style),
                    );
                    f.render_widget(p4, db_user_cols[0]);

                    let f5_style = if self.active_field == 5 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p5_line = render_text_with_cursor(
                        &self.username,
                        self.cursor_pos,
                        self.active_field == 5,
                        false,
                    );
                    let p5 = Paragraph::new(p5_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Username (e.g. lakehouse) ")
                            .border_style(f5_style),
                    );
                    f.render_widget(p5, db_user_cols[1]);

                    let pass_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(field_layout[3]);
                    let f6_style = if self.active_field == 6 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p6_line = render_text_with_cursor(
                        &self.password,
                        self.cursor_pos,
                        self.active_field == 6,
                        true,
                    );
                    let p6 = Paragraph::new(p6_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Password (Masked) ")
                            .border_style(f6_style),
                    );
                    f.render_widget(p6, pass_alias_cols[0]);

                    let f7_style = if self.active_field == 7 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p7_line = render_text_with_cursor(
                        &self.alias,
                        self.cursor_pos,
                        self.active_field == 7,
                        false,
                    );
                    let p7 = Paragraph::new(p7_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Lakehouse Catalog Alias ")
                            .border_style(f7_style),
                    );
                    f.render_widget(p7, pass_alias_cols[1]);
                }
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => {
                    let path_alias_cols = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .split(field_layout[1]);
                    let f2_style = if self.active_field == 2 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let title = if self.ducklake_backend == DuckLakeBackend::LocalFile {
                        " Catalog Metadata File (e.g. metadata.ducklake) "
                    } else {
                        " SQLite Catalog Database (e.g. metadata.db) "
                    };
                    let p2_line = render_text_with_cursor(
                        &self.path,
                        self.cursor_pos,
                        self.active_field == 2,
                        false,
                    );
                    let p2 = Paragraph::new(p2_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(title)
                            .border_style(f2_style),
                    );
                    f.render_widget(p2, path_alias_cols[0]);

                    let f3_style = if self.active_field == 3 {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    let p3_line = render_text_with_cursor(
                        &self.alias,
                        self.cursor_pos,
                        self.active_field == 3,
                        false,
                    );
                    let p3 = Paragraph::new(p3_line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Lakehouse Catalog Alias ")
                            .border_style(f3_style),
                    );
                    f.render_widget(p3, path_alias_cols[1]);
                }
            }

            // DATA_PATH layout slot
            let data_path_slot = match self.ducklake_backend {
                DuckLakeBackend::Quack => 3,
                DuckLakeBackend::PostgreSQL => 4,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 2,
            };
            let dp_style = if self.active_field == data_path_idx {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let dp_line = render_text_with_cursor(
                &self.data_path,
                self.cursor_pos,
                self.active_field == data_path_idx,
                false,
            );
            let dp_widget = Paragraph::new(dp_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Storage DATA_PATH (e.g. s3://bucket/data/ or /local/path/) ")
                    .border_style(dp_style),
            );
            f.render_widget(dp_widget, field_layout[data_path_slot]);

            // Read-Only, Disable SSL & Tip layout slot
            let ro_slot = match self.ducklake_backend {
                DuckLakeBackend::Quack => 4,
                DuckLakeBackend::PostgreSQL => 5,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 3,
            };
            if self.ducklake_backend == DuckLakeBackend::Quack {
                let toggle_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(22),
                        Constraint::Percentage(24),
                        Constraint::Percentage(54),
                    ])
                    .split(field_layout[ro_slot]);
                let ro_style = if self.active_field == ro_idx {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Read-Only",
                        if self.read_only { "X" } else { " " }
                    ))
                    .style(ro_style),
                    toggle_cols[0],
                );
                let ssl_style = if self.active_field == 8 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "[{}] Disable SSL",
                        if self.disable_ssl { "X" } else { " " }
                    ))
                    .style(ssl_style),
                    toggle_cols[1],
                );
                f.render_widget(
                    Paragraph::new("💡 Ctrl+X: S3 / RustFS secrets")
                        .style(Style::default().fg(Color::DarkGray)),
                    toggle_cols[2],
                );
            } else {
                let toggle_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(28), Constraint::Percentage(72)])
                    .split(field_layout[ro_slot]);
                let ro_style = if self.active_field == ro_idx {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Read-Only",
                        if self.read_only { "X" } else { " " }
                    ))
                    .style(ro_style),
                    toggle_cols[0],
                );
                f.render_widget(
                    Paragraph::new("💡 S3/RustFS Tip: Use Ctrl+X beforehand in IDE to configure S3 / RustFS secrets")
                        .style(Style::default().fg(Color::DarkGray)),
                    toggle_cols[1],
                );
            }

            // Error / Help layout slot
            let err_slot = match self.ducklake_backend {
                DuckLakeBackend::Quack => 5,
                DuckLakeBackend::PostgreSQL => 6,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 4,
            };
            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[err_slot],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[err_slot],
                );
            }

            // Buttons layout slot
            let btn_slot = match self.ducklake_backend {
                DuckLakeBackend::Quack => 6,
                DuckLakeBackend::PostgreSQL => 7,
                DuckLakeBackend::LocalFile | DuckLakeBackend::SQLite => 5,
            };
            let attach_btn_style = if self.active_field == submit_idx {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == cancel_idx {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ 🌊 Attach Lakehouse ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[btn_slot],
            );
        } else if is_relational {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Host & Port
                    Constraint::Length(3),         // 2: Database & Username
                    Constraint::Length(3),         // 3: Password & Alias
                    Constraint::Length(2),         // 4: Toggles
                    Constraint::Length(err_lines), // 5: Error/hint
                    Constraint::Length(3),         // 6: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let host_port_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                .split(field_layout[1]);
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1_line = render_text_with_cursor(
                &self.host,
                self.cursor_pos,
                self.active_field == 1,
                false,
            );
            let p1 = Paragraph::new(p1_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Host / Server Address ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, host_port_cols[0]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2_line = render_text_with_cursor(
                &self.port,
                self.cursor_pos,
                self.active_field == 2,
                false,
            );
            let p2 = Paragraph::new(p2_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Port ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, host_port_cols[1]);

            let db_user_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p3_line = render_text_with_cursor(
                &self.database,
                self.cursor_pos,
                self.active_field == 3,
                false,
            );
            let p3 = Paragraph::new(p3_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Name ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, db_user_cols[0]);

            let user_title = match self.provider() {
                AttachProvider::MSSQL if self.trusted_connection => " User ID (Windows Auth Active) ",
                AttachProvider::MSSQL => " User ID (e.g. sa) ",
                AttachProvider::PostgreSQL => " Username (e.g. postgres) ",
                _ => " Username (e.g. root) ",
            };
            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p4_line = render_text_with_cursor(
                &self.username,
                self.cursor_pos,
                self.active_field == 4,
                false,
            );
            let p4 = Paragraph::new(p4_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(user_title)
                    .border_style(f4_style),
            );
            f.render_widget(p4, db_user_cols[1]);

            let pass_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[3]);
            let f5_style = if self.active_field == 5 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let pass_title = if self.provider() == AttachProvider::MSSQL && self.trusted_connection {
                " Password (Not Required for Windows Auth) "
            } else {
                " Password (Masked) "
            };
            let p5_line = render_text_with_cursor(
                &self.password,
                self.cursor_pos,
                self.active_field == 5,
                true,
            );
            let p5 = Paragraph::new(p5_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(pass_title)
                    .border_style(f5_style),
            );
            f.render_widget(p5, pass_alias_cols[0]);

            let f6_style = if self.active_field == 6 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p6_line = render_text_with_cursor(
                &self.alias,
                self.cursor_pos,
                self.active_field == 6,
                false,
            );
            let p6 = Paragraph::new(p6_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f6_style),
            );
            f.render_widget(p6, pass_alias_cols[1]);

            let is_mssql = self.provider() == AttachProvider::MSSQL;
            if is_mssql {
                let toggle_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(24),
                        Constraint::Percentage(28),
                        Constraint::Percentage(24),
                        Constraint::Percentage(24),
                    ])
                    .split(field_layout[4]);

                let f7_style = if self.active_field == 7 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Read-Only",
                        if self.read_only { "X" } else { " " }
                    ))
                    .style(f7_style),
                    toggle_cols[0],
                );

                let f8_style = if self.active_field == 8 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Windows Auth",
                        if self.trusted_connection { "X" } else { " " }
                    ))
                    .style(f8_style),
                    toggle_cols[1],
                );

                let f9_style = if self.active_field == 9 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Disable SSL",
                        if self.disable_ssl { "X" } else { " " }
                    ))
                    .style(f9_style),
                    toggle_cols[2],
                );

                let f10_style = if self.active_field == 10 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Trust Cert",
                        if self.insecure { "X" } else { " " }
                    ))
                    .style(f10_style),
                    toggle_cols[3],
                );
            } else {
                let toggle_cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(33),
                        Constraint::Percentage(33),
                        Constraint::Percentage(34),
                    ])
                    .split(field_layout[4]);
                let f7_style = if self.active_field == 7 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Read-Only",
                        if self.read_only { "X" } else { " " }
                    ))
                    .style(f7_style),
                    toggle_cols[0],
                );

                let f8_style = if self.active_field == 8 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Disable SSL",
                        if self.disable_ssl { "X" } else { " " }
                    ))
                    .style(f8_style),
                    toggle_cols[1],
                );

                let f9_style = if self.active_field == 9 {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "  [{}] Insecure / Trust",
                        if self.insecure { "X" } else { " " }
                    ))
                    .style(f9_style),
                    toggle_cols[2],
                );
            }

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[5],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[5],
                );
            }

            let attach_btn_style = if (is_mssql && self.active_field == 11)
                || (!is_mssql && self.active_field == 10)
            {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if (is_mssql && self.active_field == 12)
                || (!is_mssql && self.active_field == 11)
            {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[6],
            );
        } else if self.provider().is_quack() {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Host & Port
                    Constraint::Length(3),         // 2: Token & Alias
                    Constraint::Length(2),         // 3: Toggles
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let host_port_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                .split(field_layout[1]);
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1_line = render_text_with_cursor(
                &self.host,
                self.cursor_pos,
                self.active_field == 1,
                false,
            );
            let p1 = Paragraph::new(p1_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Remote Quack Host / IP (e.g. localhost) ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, host_port_cols[0]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2_line = render_text_with_cursor(
                &self.port,
                self.cursor_pos,
                self.active_field == 2,
                false,
            );
            let p2 = Paragraph::new(p2_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Port ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, host_port_cols[1]);

            let tok_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p3_line = render_text_with_cursor(
                &self.password,
                self.cursor_pos,
                self.active_field == 3,
                true,
            );
            let p3 = Paragraph::new(p3_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Auth Token (Optional, Masked) ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, tok_alias_cols[0]);

            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p4_line = render_text_with_cursor(
                &self.alias,
                self.cursor_pos,
                self.active_field == 4,
                false,
            );
            let p4 = Paragraph::new(p4_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f4_style),
            );
            f.render_widget(p4, tok_alias_cols[1]);

            let toggle_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                ])
                .split(field_layout[3]);
            let f5_style = if self.active_field == 5 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f5_style),
                toggle_cols[0],
            );

            let f6_style = if self.active_field == 6 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Disable SSL",
                    if self.disable_ssl { "X" } else { " " }
                ))
                .style(f6_style),
                toggle_cols[1],
            );

            let f7_style = if self.active_field == 7 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Insecure / Trust",
                    if self.insecure { "X" } else { " " }
                ))
                .style(f7_style),
                toggle_cols[2],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 8 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 9 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        } else if self.provider().is_motherduck() {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Database Name
                    Constraint::Length(3),         // 2: Token & Alias
                    Constraint::Length(2),         // 3: Toggles
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1_line = render_text_with_cursor(
                &self.path,
                self.cursor_pos,
                self.active_field == 1,
                false,
            );
            let p1 = Paragraph::new(p1_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" MotherDuck Database Name (e.g. my_db or md:my_db) ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, field_layout[1]);

            let tok_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2_line = render_text_with_cursor(
                &self.password,
                self.cursor_pos,
                self.active_field == 2,
                true,
            );
            let p2 = Paragraph::new(p2_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Service Token (Optional, Masked) ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, tok_alias_cols[0]);

            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p3_line = render_text_with_cursor(
                &self.alias,
                self.cursor_pos,
                self.active_field == 3,
                false,
            );
            let p3 = Paragraph::new(p3_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, tok_alias_cols[1]);

            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f4_style),
                field_layout[3],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 5 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 6 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        } else {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: File Path
                    Constraint::Length(3),         // 2: Alias
                    Constraint::Length(2),         // 3: Read-Only toggle
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let path_title = if self.provider() == AttachProvider::DuckDB {
                " Database File Path (e.g. data.duckdb or :memory:) "
            } else {
                " SQLite Database File Path (.db, .sqlite) "
            };
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1_line = render_text_with_cursor(
                &self.path,
                self.cursor_pos,
                self.active_field == 1,
                false,
            );
            let p1 = Paragraph::new(p1_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(path_title)
                    .border_style(f1_style),
            );
            f.render_widget(p1, field_layout[1]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2_line = render_text_with_cursor(
                &self.alias,
                self.cursor_pos,
                self.active_field == 2,
                false,
            );
            let p2 = Paragraph::new(p2_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, field_layout[2]);

            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f3_style),
                field_layout[3],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 4 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 5 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        }
    }

    pub(crate) fn execute_attach(&mut self, conn: &Connection) -> Option<String> {
        match self.build_connection_target() {
            Err(err) => {
                self.error_msg = Some(err);
            }
            Ok((safe_path, alias)) => {
                let provider = self.provider();
                let ext_res = match provider {
                    AttachProvider::DuckLake => {
                        let opt_token = if self.password.trim().is_empty() {
                            None
                        } else {
                            Some(self.password.trim())
                        };
                        crate::engine::prepare_ducklake_connection(
                            conn,
                            &safe_path,
                            &self.data_path,
                            opt_token,
                            None,
                        )
                    }
                    AttachProvider::SQLite => {
                        crate::engine::ensure_extension_loaded(conn, "sqlite")
                    }
                    AttachProvider::PostgreSQL => {
                        crate::engine::ensure_extension_loaded(conn, "postgres")
                    }
                    AttachProvider::MySQL => {
                        crate::engine::ensure_extension_loaded(conn, "mysql")
                    }
                    AttachProvider::MSSQL => {
                        crate::engine::ensure_extension_loaded(conn, "mssql")
                    }
                    AttachProvider::MotherDuck => {
                        crate::engine::ensure_extension_loaded(conn, "motherduck")
                    }
                    AttachProvider::Quack => crate::engine::load_quack_extension(conn),
                    AttachProvider::DuckDB => Ok(()),
                };

                if let Err(e) = ext_res {
                    self.error_msg = Some(format!(
                        "Failed to load required extension for {}: {}",
                        provider.name(),
                        e
                    ));
                } else {
                    if provider == AttachProvider::MotherDuck
                        && !self.password.trim().is_empty()
                    {
                        let safe_token = self.password.trim().replace('\'', "''");
                        let _ = conn.execute(
                            &format!("SET motherduck_token='{}';", safe_token),
                            [],
                        );
                    }
                    if provider == AttachProvider::Quack && !self.password.trim().is_empty() {
                        let safe_token = self.password.trim().replace('\'', "''");
                        let _ = conn.execute(
                            &format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token),
                            [],
                        );
                    }

                    let quoted_alias = format!("\"{}\"", alias.replace('"', "\"\""));
                    let mut attach_sql = format!("ATTACH '{}' AS {}", safe_path, quoted_alias);
                    let mut opts = Vec::new();
                    match provider {
                        AttachProvider::DuckLake => {
                            if !self.data_path.trim().is_empty() {
                                opts.push(format!(
                                    "DATA_PATH '{}'",
                                    self.data_path.trim().replace('\'', "''")
                                ));
                            }
                            if self.ducklake_backend == DuckLakeBackend::Quack {
                                let quack_dsn = safe_path
                                    .strip_prefix("ducklake:")
                                    .unwrap_or(&safe_path);
                                if self.disable_ssl
                                    || self.insecure
                                    || crate::engine::should_disable_quack_ssl(quack_dsn, false)
                                {
                                    opts.push("METADATA_PARAMETERS MAP {'DISABLE_SSL': true}".to_string());
                                }
                            }
                        }
                        AttachProvider::SQLite => opts.push("TYPE SQLITE".to_string()),
                        AttachProvider::PostgreSQL => {
                            opts.push("TYPE POSTGRES".to_string())
                        }
                        AttachProvider::MySQL => opts.push("TYPE MYSQL".to_string()),
                        AttachProvider::MSSQL => opts.push("TYPE MSSQL".to_string()),
                        AttachProvider::Quack => {
                            if self.disable_ssl
                                || self.insecure
                                || crate::engine::should_disable_quack_ssl(&safe_path, false)
                            {
                                opts.push("DISABLE_SSL true".to_string());
                            }
                        }
                        _ => {}
                    }
                    if self.read_only {
                        opts.push("READ_ONLY".to_string());
                    }
                    if !opts.is_empty() {
                        attach_sql.push_str(&format!(" ({})", opts.join(", ")));
                    }
                    attach_sql.push(';');

                    match conn.execute(&attach_sql, []) {
                        Ok(_) => {
                            self.is_open = false;
                            return Some(alias);
                        }
                        Err(e) => {
                            self.error_msg = Some(format!(
                                "Attach Error: {}",
                                crate::engine::redact_connection_credentials(&e.to_string())
                            ));
                        }
                    }
                }
            }
        }
        None
    }

    /// Processes a keyboard event in the Attach Database modal.
    /// Returns `Some(attached_alias)` if a database was attached successfully.
    pub(crate) fn handle_key(&mut self, key: KeyEvent, conn: &Connection) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab {
            self.next_field();
        } else if key.code == KeyCode::BackTab {
            self.prev_field();
        } else if key.code == KeyCode::Down {
            self.nav_down();
        } else if key.code == KeyCode::Up {
            self.nav_up();
        } else if self.is_text_field() {
            match key.code {
                KeyCode::Left => {
                    self.cursor_pos = self.cursor_pos.saturating_sub(1);
                }
                KeyCode::Right => {
                    let len = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
                    self.cursor_pos = (self.cursor_pos + 1).min(len);
                }
                KeyCode::Home => {
                    self.cursor_pos = 0;
                }
                KeyCode::End => {
                    self.sync_cursor_to_end();
                }
                KeyCode::Backspace => {
                    self.handle_backspace();
                }
                KeyCode::Delete => {
                    self.handle_delete();
                }
                KeyCode::Enter => {
                    return self.execute_attach(conn);
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        self.handle_char(c);
                    }
                }
                _ => {}
            }
        } else {
            if key.code == KeyCode::Left
                || ((self.active_field == 0
                    || (self.provider() == AttachProvider::DuckLake && self.active_field == 1))
                    && key.code == KeyCode::Char('h'))
            {
                self.nav_left();
            } else if key.code == KeyCode::Right
                || ((self.active_field == 0
                    || (self.provider() == AttachProvider::DuckLake && self.active_field == 1))
                    && key.code == KeyCode::Char('l'))
            {
                self.nav_right();
            } else if key.code == KeyCode::Char(' ') && self.is_toggle_field() {
                self.toggle_active();
            } else if key.code == KeyCode::Enter {
                if self.is_cancel() {
                    self.is_open = false;
                } else if self.is_toggle_field() {
                    self.toggle_active();
                } else {
                    return self.execute_attach(conn);
                }
            }
        }
        None
    }
}
