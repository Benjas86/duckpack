use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use std::collections::HashSet;
use std::path::Path;

use super::input::{
    delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
    render_text_with_cursor,
};

/// Supported cloud storage providers for secrets and bucket exploration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CloudProvider {
    S3,
    CloudflareR2,
    MinIO,
    Wasabi,
    CustomS3,
}

impl CloudProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            CloudProvider::S3 => "AWS S3",
            CloudProvider::CloudflareR2 => "Cloudflare R2",
            CloudProvider::MinIO => "MinIO (Self-hosted)",
            CloudProvider::Wasabi => "Wasabi Cloud",
            CloudProvider::CustomS3 => "Custom S3 Endpoint",
        }
    }
}

/// State for the Cloud Secrets & S3 modal dialog (Ctrl+K).
#[derive(Clone, Debug)]
pub(crate) struct SecretModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: S3, 1: R2, 2: MinIO, 3: Wasabi, 4: Custom
    pub(crate) name: String,
    pub(crate) key_id: String,
    pub(crate) secret_key: String,
    pub(crate) region: String,
    pub(crate) endpoint: String,
    pub(crate) bucket: String,
    pub(crate) cursor_pos: usize,
    pub(crate) use_ssl: bool,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: provider, 1: name, 2: key_id, 3: secret_key, 4: region, 5: endpoint, 6: bucket, 7: use_ssl, 8: save_to_env, 9: submit, 10: cancel
    pub(crate) error_msg: Option<String>,
}

impl SecretModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            provider_index: 0,
            name: "s3_secret".to_string(),
            key_id: String::new(),
            secret_key: String::new(),
            region: "us-east-1".to_string(),
            endpoint: String::new(),
            bucket: String::new(),
            cursor_pos: 0,
            use_ssl: true,
            save_to_env: true,
            active_field: 2,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.name = "s3_secret".to_string();
        self.key_id.clear();
        self.secret_key.clear();
        self.region = "us-east-1".to_string();
        self.endpoint.clear();
        self.bucket.clear();
        self.cursor_pos = 0;
        self.use_ssl = true;
        self.save_to_env = true;
        self.active_field = 2;
        self.error_msg = None;
    }

    pub(crate) fn is_text_field(&self) -> bool {
        (1..=6).contains(&self.active_field)
    }

    pub(crate) fn active_text(&self) -> Option<&str> {
        match self.active_field {
            1 => Some(&self.name),
            2 => Some(&self.key_id),
            3 => Some(&self.secret_key),
            4 => Some(&self.region),
            5 => Some(&self.endpoint),
            6 => Some(&self.bucket),
            _ => None,
        }
    }

    pub(crate) fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.active_field {
            1 => Some(&mut self.name),
            2 => Some(&mut self.key_id),
            3 => Some(&mut self.secret_key),
            4 => Some(&mut self.region),
            5 => Some(&mut self.endpoint),
            6 => Some(&mut self.bucket),
            _ => None,
        }
    }

    pub(crate) fn sync_cursor_to_end(&mut self) {
        self.cursor_pos = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
    }

    pub(crate) fn provider(&self) -> CloudProvider {
        match self.provider_index {
            1 => CloudProvider::CloudflareR2,
            2 => CloudProvider::MinIO,
            3 => CloudProvider::Wasabi,
            4 => CloudProvider::CustomS3,
            _ => CloudProvider::S3,
        }
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 5;
        match self.provider() {
            CloudProvider::S3 => {
                if self.endpoint == "localhost:9000"
                    || self.endpoint == "s3.wasabisys.com"
                    || self.endpoint.contains(".r2.cloudflarestorage.com")
                {
                    self.endpoint.clear();
                }
                if self.region == "auto" {
                    self.region = "us-east-1".to_string();
                }
                self.use_ssl = true;
            }
            CloudProvider::CloudflareR2 => {
                if self.endpoint.is_empty()
                    || self.endpoint == "localhost:9000"
                    || self.endpoint == "s3.wasabisys.com"
                {
                    self.endpoint = "<account_id>.r2.cloudflarestorage.com".to_string();
                }
                self.region = "auto".to_string();
                self.use_ssl = true;
            }
            CloudProvider::MinIO => {
                self.endpoint = "localhost:9000".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = false;
            }
            CloudProvider::Wasabi => {
                self.endpoint = "s3.wasabisys.com".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = true;
            }
            CloudProvider::CustomS3 => {
                self.use_ssl = true;
            }
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 72.min(f.area().width.saturating_sub(4));
        let popup_height = 26.min(f.area().height.saturating_sub(2));
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
            .title(" 🔐 Cloud Secrets & S3 Buckets (Ctrl+K) ")
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let field_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Provider
                Constraint::Length(3), // 1: Name
                Constraint::Length(3), // 2: Key ID
                Constraint::Length(3), // 3: Secret Key
                Constraint::Length(3), // 4: Region & Endpoint
                Constraint::Length(3), // 5: Bucket
                Constraint::Length(2), // 6: SSL & Save-to-.env toggles
                Constraint::Length(1), // 7: Hint / Error
                Constraint::Length(3), // 8: Buttons
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
                .title(" Storage Provider ")
                .border_style(f0_style),
        );
        f.render_widget(p0, field_layout[0]);

        let f1_style = if self.active_field == 1 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p1 = Paragraph::new(render_text_with_cursor(
            &self.name,
            self.cursor_pos,
            self.active_field == 1,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" DuckDB Secret Identifier ")
                .border_style(f1_style),
        );
        f.render_widget(p1, field_layout[1]);

        let f2_style = if self.active_field == 2 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p2 = Paragraph::new(render_text_with_cursor(
            &self.key_id,
            self.cursor_pos,
            self.active_field == 2,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Access Key ID (AWS_ACCESS_KEY_ID) ")
                .border_style(f2_style),
        );
        f.render_widget(p2, field_layout[2]);

        let f3_style = if self.active_field == 3 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p3 = Paragraph::new(render_text_with_cursor(
            &self.secret_key,
            self.cursor_pos,
            self.active_field == 3,
            true,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Secret Access Key (AWS_SECRET_ACCESS_KEY, Masked) ")
                .border_style(f3_style),
        );
        f.render_widget(p3, field_layout[3]);

        let reg_end_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(field_layout[4]);

        let f4_reg_style = if self.active_field == 4 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p_reg = Paragraph::new(render_text_with_cursor(
            &self.region,
            self.cursor_pos,
            self.active_field == 4,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Region ")
                .border_style(f4_reg_style),
        );
        f.render_widget(p_reg, reg_end_cols[0]);

        let f5_end_style = if self.active_field == 5 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p_end = Paragraph::new(render_text_with_cursor(
            &self.endpoint,
            self.cursor_pos,
            self.active_field == 5,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Custom Endpoint (Optional) ")
                .border_style(f5_end_style),
        );
        f.render_widget(p_end, reg_end_cols[1]);

        let f6_style = if self.active_field == 6 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p6 = Paragraph::new(render_text_with_cursor(
            &self.bucket,
            self.cursor_pos,
            self.active_field == 6,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Default Bucket to Explore (e.g. s3://my-bucket) ")
                .border_style(f6_style),
        );
        f.render_widget(p6, field_layout[5]);

        let toggle_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(field_layout[6]);

        let f7_ssl_style = if self.active_field == 7 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let ssl_label = format!(
            "  [{}] Use SSL / HTTPS (Space)",
            if self.use_ssl { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(ssl_label).style(f7_ssl_style), toggle_cols[0]);

        let f8_env_style = if self.active_field == 8 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let env_label = format!(
            "  [{}] Persist in .env (Space)",
            if self.save_to_env { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(env_label).style(f8_env_style), toggle_cols[1]);

        if let Some(ref err) = self.error_msg {
            f.render_widget(
                Paragraph::new(format!(" ⚠️ {}", err))
                    .style(Style::default().fg(Color::LightRed))
                    .wrap(Wrap { trim: true }),
                field_layout[7],
            );
        } else {
            f.render_widget(
                Paragraph::new(
                    "  Tab: Next Field | Space: Toggle | Enter: Save Secret | Esc: Cancel",
                )
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
                field_layout[7],
            );
        }

        let save_btn_style = if self.active_field == 9 {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let cancel_btn_style = if self.active_field == 10 {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Save & Attach Secret ] ", save_btn_style),
            Span::raw("    "),
            Span::styled(" [ Cancel ] ", cancel_btn_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            field_layout[8],
        );
    }

    /// Processes a keyboard event in the Cloud Secret modal.
    /// Returns `Some(status_msg)` if a secret was saved and the explorer should refresh.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
            self.active_field = (self.active_field + 1) % 11;
            self.sync_cursor_to_end();
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                10
            } else {
                self.active_field - 1
            };
            self.sync_cursor_to_end();
        } else if self.active_field == 0
            && (key.code == KeyCode::Left || key.code == KeyCode::Char('h'))
        {
            let prev = if self.provider_index == 0 {
                4
            } else {
                self.provider_index - 1
            };
            self.set_provider(prev);
        } else if self.active_field == 0
            && (key.code == KeyCode::Right || key.code == KeyCode::Char('l'))
        {
            let next = (self.provider_index + 1) % 5;
            self.set_provider(next);
        } else if self.active_field >= 9
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 9 { 10 } else { 9 };
        } else if self.is_text_field() && key.code == KeyCode::Left {
            self.cursor_pos = self.cursor_pos.saturating_sub(1);
        } else if self.is_text_field() && key.code == KeyCode::Right {
            let len = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
            self.cursor_pos = (self.cursor_pos + 1).min(len);
        } else if self.is_text_field() && key.code == KeyCode::Home {
            self.cursor_pos = 0;
        } else if self.is_text_field() && key.code == KeyCode::End {
            self.sync_cursor_to_end();
        } else if self.is_text_field() && key.code == KeyCode::Backspace {
            let mut cur = self.cursor_pos;
            if let Some(t) = self.active_text_mut() {
                delete_char_before_cursor(t, &mut cur);
            }
            self.cursor_pos = cur;
        } else if self.is_text_field() && key.code == KeyCode::Delete {
            let cur = self.cursor_pos;
            if let Some(t) = self.active_text_mut() {
                delete_char_at_cursor(t, cur);
            }
        } else if key.code == KeyCode::Char(' ') && self.active_field == 7 {
            self.use_ssl = !self.use_ssl;
        } else if key.code == KeyCode::Char(' ') && self.active_field == 8 {
            self.save_to_env = !self.save_to_env;
        } else if key.code == KeyCode::Enter {
            if self.active_field == 10 {
                self.is_open = false;
            } else if self.active_field == 7 {
                self.use_ssl = !self.use_ssl;
            } else if self.active_field == 8 {
                self.save_to_env = !self.save_to_env;
            } else {
                let safe_name = self.name.trim();

                if safe_name.is_empty() {
                    self.error_msg = Some("Secret name cannot be empty.".to_string());
                } else if !safe_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    self.error_msg = Some(
                        "Secret name must contain only alphanumeric characters and underscores."
                            .to_string(),
                    );
                } else {
                    let _ = conn.execute("INSTALL httpfs; LOAD httpfs;", []);
                    let secret_body = crate::engine::build_s3_secret_body(
                        &self.key_id,
                        &self.secret_key,
                        &self.region,
                        &self.endpoint,
                        self.use_ssl,
                    );
                    let persistent_sql = format!(
                        "CREATE OR REPLACE PERSISTENT SECRET {} ({});",
                        safe_name, secret_body
                    );
                    let create_res = conn.execute(&persistent_sql, []).or_else(|_| {
                        let temp_sql =
                            format!("CREATE OR REPLACE SECRET {} ({});", safe_name, secret_body);
                        conn.execute(&temp_sql, [])
                    });

                    match create_res {
                        Ok(_) => {
                            if self.save_to_env {
                                let _ = crate::engine::save_cloud_secret_to_env(
                                    project_dir,
                                    self.key_id.trim(),
                                    self.secret_key.trim(),
                                    self.region.trim(),
                                    self.endpoint.trim(),
                                    self.bucket.trim(),
                                );
                            }

                            let mut bucket = self.bucket.trim().to_string();
                            if !bucket.is_empty() {
                                if !bucket.starts_with("s3://")
                                    && !bucket.starts_with("r2://")
                                    && !bucket.starts_with("gcs://")
                                {
                                    bucket = format!("s3://{}", bucket);
                                }
                                unsafe {
                                    std::env::set_var("DUCKPACK_S3_BUCKET", &bucket);
                                }
                            }

                            self.is_open = false;
                            return Some(format!(
                                "Configured cloud secret '{}' successfully.",
                                safe_name
                            ));
                        }
                        Err(e) => {
                            self.error_msg = Some(format!("Secret Error: {}", e));
                        }
                    }
                }
            }
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) && self.is_text_field() {
                let mut cur = self.cursor_pos;
                if let Some(t) = self.active_text_mut() {
                    insert_char_at_cursor(t, &mut cur, c);
                }
                self.cursor_pos = cur;
            }
        }
        None
    }
}

/// Populates `SecretModalState` with existing secret configuration from DuckDB and `.env` for in-place editing.
pub(crate) fn populate_secret_modal_for_edit(
    conn: &duckdb::Connection,
    project_dir: &std::path::Path,
    secret_name: &str,
    modal: &mut SecretModalState,
) {
    modal.is_open = true;
    modal.name = secret_name.to_string();
    modal.active_field = 2; // Focus on Key ID by default
    modal.error_msg = None;

    // 1. Query DuckDB's duckdb_secrets() to extract existing config
    let sql = format!(
        "SELECT secret_string FROM duckdb_secrets() WHERE name = '{}';",
        secret_name.replace('\'', "''")
    );
    let mut secret_string_opt = None;
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(mut rows) = stmt.query([]) {
            if let Ok(Some(row)) = rows.next() {
                secret_string_opt = row.get::<_, String>(0).ok();
            }
        }
    }
    if secret_string_opt.is_none() {
        let fallback_sql = format!(
            "SELECT secret_string FROM duckdb_secrets WHERE name = '{}';",
            secret_name.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&fallback_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                if let Ok(Some(row)) = rows.next() {
                    secret_string_opt = row.get::<_, String>(0).ok();
                }
            }
        }
    }
    if let Some(secret_string) = secret_string_opt {
        for part in secret_string.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                match k.trim() {
                    "key_id" => modal.key_id = v.trim().to_string(),
                    "region" => modal.region = v.trim().to_string(),
                    "endpoint" => modal.endpoint = v.trim().to_string(),
                    "use_ssl" => modal.use_ssl = v.trim().eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }
    }

    // 2. Read .env file for secrets / bucket if available
    let env_path = project_dir.join(".env");
    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"').trim_matches('\'');
                match key {
                    "AWS_ACCESS_KEY_ID" if modal.key_id.is_empty() => {
                        modal.key_id = val.to_string()
                    }
                    "AWS_SECRET_ACCESS_KEY" => modal.secret_key = val.to_string(),
                    "AWS_REGION" if modal.region.is_empty() => modal.region = val.to_string(),
                    "AWS_ENDPOINT" if modal.endpoint.is_empty() => modal.endpoint = val.to_string(),
                    "DUCKPACK_S3_BUCKET" if modal.bucket.is_empty() => {
                        modal.bucket = val.to_string()
                    }
                    _ => {}
                }
            }
        }
    }

    // 3. Infer provider index from endpoint
    let ep_lower = modal.endpoint.to_lowercase();
    if ep_lower.contains("r2.cloudflarestorage.com") {
        modal.provider_index = 1;
    } else if ep_lower.contains("localhost:9000")
        || ep_lower.contains("127.0.0.1:9000")
        || ep_lower.contains("minio")
        || ep_lower.contains("rustfs")
    {
        modal.provider_index = 2;
    } else if ep_lower.contains("wasabisys.com") {
        modal.provider_index = 3;
    } else if !modal.endpoint.is_empty() {
        modal.provider_index = 4;
    } else {
        modal.provider_index = 0;
    }
    modal.sync_cursor_to_end();
}

/// State for the Add Cloud Bucket modal dialog.
#[derive(Clone, Debug)]
pub(crate) struct AddBucketModalState {
    pub(crate) is_open: bool,
    pub(crate) initial_uri: Option<String>,
    pub(crate) uri: String,
    pub(crate) cursor_pos: usize,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: uri, 1: save_to_env, 2: submit, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl AddBucketModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            initial_uri: None,
            uri: String::new(),
            cursor_pos: 0,
            save_to_env: true,
            active_field: 0,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.initial_uri = None;
        self.uri.clear();
        self.cursor_pos = 0;
        self.save_to_env = true;
        self.active_field = 0;
        self.error_msg = None;
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 64.min(f.area().width.saturating_sub(4));
        let popup_height = 14.min(f.area().height.saturating_sub(2));
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
            .title(" 🪣 Add S3 / Cloud Bucket ")
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let field_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Bucket URI
                Constraint::Length(2), // 1: Persist in .env toggle
                Constraint::Length(2), // 2: Hint / error
                Constraint::Length(3), // 3: Buttons
            ])
            .split(inner_area);

        let f0_style = if self.active_field == 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p0 = Paragraph::new(render_text_with_cursor(
            &self.uri,
            self.cursor_pos,
            self.active_field == 0,
            false,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Bucket URI or Name (e.g. s3://my-lake) ")
                .border_style(f0_style),
        );
        f.render_widget(p0, field_layout[0]);

        let f1_style = if self.active_field == 1 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let env_label = format!(
            "  [{}] Persist to .env (Space to toggle)",
            if self.save_to_env { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(env_label).style(f1_style), field_layout[1]);

        if let Some(ref err) = self.error_msg {
            f.render_widget(
                Paragraph::new(format!(" ⚠️ {}", err))
                    .style(Style::default().fg(Color::LightRed))
                    .wrap(Wrap { trim: true }),
                field_layout[2],
            );
        } else {
            f.render_widget(
                Paragraph::new(
                    "  Tab: Next Field | Space: Toggle | Enter: Add Bucket | Esc: Cancel",
                )
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
                field_layout[2],
            );
        }

        let add_btn_style = if self.active_field == 2 {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let cancel_btn_style = if self.active_field == 3 {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Add Bucket ] ", add_btn_style),
            Span::raw("    "),
            Span::styled(" [ Cancel ] ", cancel_btn_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            field_layout[3],
        );
    }

    /// Processes a keyboard event in the Add Bucket modal.
    /// Returns `Some(status_msg)` if a bucket was added and the explorer should refresh.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        project_dir: &Path,
        expanded_groups: &mut HashSet<String>,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
            self.active_field = (self.active_field + 1) % 4;
            if self.active_field == 0 {
                self.cursor_pos = self.uri.chars().count();
            }
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                3
            } else {
                self.active_field - 1
            };
            if self.active_field == 0 {
                self.cursor_pos = self.uri.chars().count();
            }
        } else if self.active_field >= 2
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 2 { 3 } else { 2 };
        } else if self.active_field == 0 && key.code == KeyCode::Left {
            self.cursor_pos = self.cursor_pos.saturating_sub(1);
        } else if self.active_field == 0 && key.code == KeyCode::Right {
            self.cursor_pos = (self.cursor_pos + 1).min(self.uri.chars().count());
        } else if self.active_field == 0 && key.code == KeyCode::Home {
            self.cursor_pos = 0;
        } else if self.active_field == 0 && key.code == KeyCode::End {
            self.cursor_pos = self.uri.chars().count();
        } else if key.code == KeyCode::Char(' ') && self.active_field == 1 {
            self.save_to_env = !self.save_to_env;
        } else if key.code == KeyCode::Enter {
            if self.active_field == 3 {
                self.is_open = false;
            } else if self.active_field == 1 {
                self.save_to_env = !self.save_to_env;
            } else {
                let clean_uri = self.uri.trim();
                if clean_uri.is_empty() {
                    self.error_msg = Some("Bucket URI or name cannot be empty.".to_string());
                } else {
                    let mut normalized = clean_uri.to_string();
                    if !normalized.starts_with("s3://")
                        && !normalized.starts_with("r2://")
                        && !normalized.starts_with("gcs://")
                        && !normalized.starts_with("azure://")
                    {
                        normalized = format!("s3://{}", normalized);
                    }

                    if let Some(ref old_uri) = self.initial_uri {
                        if old_uri != &normalized {
                            expanded_groups.remove(&format!("bucket:{}", old_uri));
                            unsafe {
                                let current = std::env::var("DUCKPACK_S3_BUCKETS")
                                    .or_else(|_| std::env::var("DUCKPACK_S3_BUCKET"))
                                    .unwrap_or_default();
                                let mut new_parts: Vec<&str> = current
                                    .split(',')
                                    .map(|s| s.trim())
                                    .filter(|s| {
                                        !s.is_empty()
                                            && *s != old_uri.as_str()
                                            && *s != old_uri.trim_start_matches("s3://")
                                    })
                                    .collect();
                                if !new_parts.contains(&normalized.as_str()) {
                                    new_parts.push(&normalized);
                                }
                                let joined = new_parts.join(",");
                                std::env::set_var("DUCKPACK_S3_BUCKETS", &joined);
                                std::env::set_var("DUCKPACK_S3_BUCKET", &joined);
                            }
                        }
                    }

                    if self.save_to_env {
                        let _ = crate::engine::append_cloud_bucket_to_env(project_dir, &normalized);
                    }

                    unsafe {
                        let current = std::env::var("DUCKPACK_S3_BUCKETS")
                            .or_else(|_| std::env::var("DUCKPACK_S3_BUCKET"))
                            .unwrap_or_default();
                        let new_var = if current.is_empty() {
                            normalized.clone()
                        } else if !current.split(',').any(|b| b.trim() == normalized) {
                            format!("{},{}", current, normalized)
                        } else {
                            current
                        };
                        std::env::set_var("DUCKPACK_S3_BUCKETS", &new_var);
                        std::env::set_var("DUCKPACK_S3_BUCKET", &new_var);
                    }

                    let bucket_key = format!("bucket:{}", normalized);
                    expanded_groups.insert("Secrets & Cloud Buckets".to_string());
                    expanded_groups.insert(bucket_key);

                    self.is_open = false;
                    return Some(format!("Added bucket '{}' to explorer.", normalized));
                }
            }
        } else if key.code == KeyCode::Backspace && self.active_field == 0 {
            delete_char_before_cursor(&mut self.uri, &mut self.cursor_pos);
        } else if key.code == KeyCode::Delete && self.active_field == 0 {
            delete_char_at_cursor(&mut self.uri, self.cursor_pos);
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) && self.active_field == 0 {
                insert_char_at_cursor(&mut self.uri, &mut self.cursor_pos, c);
            }
        }
        None
    }
}
