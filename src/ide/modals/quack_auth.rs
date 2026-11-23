use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};
use std::path::Path;

use crate::ide::modals::input::{
    delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
    render_text_with_cursor,
};
use crate::quack_auth::{
    cancel_quack_session_query, deploy_quack_rbac_now, export_quack_rbac_bootstrap_script,
    export_quack_rbac_post_start_script, generate_and_save_user_token,
    load_current_quack_identity, load_live_quack_rbac_enabled, load_live_quack_sessions,
    load_live_quack_users, load_quack_auth_config, resolve_user_token, save_quack_auth_config,
    set_live_quack_user_enabled, set_live_quack_user_role, set_quack_session_override,
    validate_quack_auth_config, validate_safe_username, LiveQuackSession, LiveQuackUser,
    QuackAuthConfig, QuackAuthUser, QuackRole,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuackAuthTab {
    LiveSessions,
    Users,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingBreakGlass {
    SetEnabled {
        username: String,
        enabled: bool,
    },
    SetRole {
        username: String,
        from_role: String,
        to_role: QuackRole,
    },
}

/// Interactive `Ctrl+U` modal for Quack RBAC user management and live session monitoring.
#[derive(Clone, Debug)]
pub(crate) struct QuackAuthModalState {
    pub(crate) is_open: bool,
    pub(crate) active_tab: QuackAuthTab,
    pub(crate) quack_alias: Option<String>,
    pub(crate) targets: Vec<crate::workspace::QuackControlTarget>,
    pub(crate) target_index: usize,
    pub(crate) env_profile: Option<String>,
    pub(crate) sessions: Vec<LiveQuackSession>,
    pub(crate) session_index: usize,
    pub(crate) live_server_users: Vec<LiveQuackUser>,
    pub(crate) live_rbac_enabled: Option<bool>,
    pub(crate) current_sid: Option<String>,
    pub(crate) current_username: Option<String>,
    pub(crate) access_denied: bool,
    pending_break_glass: Option<PendingBreakGlass>,
    pub(crate) config: QuackAuthConfig,
    pub(crate) has_config_on_disk: bool,
    pub(crate) user_index: usize,
    // Inline "Add User" sub-form state
    pub(crate) adding_user: bool,
    pub(crate) new_username: String,
    pub(crate) new_role: QuackRole,
    pub(crate) new_token_env: String,
    pub(crate) new_client_id: String,
    pub(crate) add_field: usize, // 0=username, 1=role, 2=token_env, 3=client_id
    pub(crate) cursor_pos: usize,
    pub(crate) status_msg: Option<String>,
    pub(crate) error_msg: Option<String>,
}

impl QuackAuthModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            active_tab: QuackAuthTab::LiveSessions,
            quack_alias: None,
            targets: Vec::new(),
            target_index: 0,
            env_profile: None,
            sessions: Vec::new(),
            session_index: 0,
            live_server_users: Vec::new(),
            live_rbac_enabled: None,
            current_sid: None,
            current_username: None,
            access_denied: false,
            pending_break_glass: None,
            config: QuackAuthConfig {
                enabled: true,
                ..Default::default()
            },
            has_config_on_disk: false,
            user_index: 0,
            adding_user: false,
            new_username: String::new(),
            new_role: QuackRole::Reader,
            new_token_env: String::new(),
            new_client_id: String::new(),
            add_field: 0,
            cursor_pos: 0,
            status_msg: None,
            error_msg: None,
        }
    }

    /// True when Tab 2 is showing remotely managed server users from `_duckpack_auth.quack_users`
    /// (read-only audit view with guarded break-glass actions).
    fn is_remote_audit_mode(&self) -> bool {
        self.config.users.is_empty() && !self.live_server_users.is_empty()
    }

    /// True when the remote Quack server rejected RBAC inspection with Authorization/Authentication
    /// failure and this project has no local `[quack_auth]` config to manage offline.
    fn is_admin_locked_view(&self) -> bool {
        self.access_denied && !self.has_config_on_disk
    }

    fn is_current_username(&self, username: &str) -> bool {
        self.current_username
            .as_deref()
            .map(|me| me.eq_ignore_ascii_case(username))
            .unwrap_or(false)
    }

    fn is_current_sid(&self, sid: &str) -> bool {
        self.current_sid
            .as_deref()
            .map(|me| me.eq_ignore_ascii_case(sid))
            .unwrap_or(false)
    }

    pub(crate) fn active_target(&self) -> Option<&crate::workspace::QuackControlTarget> {
        self.targets.get(self.target_index)
    }

    fn sync_active_quack_alias(&mut self) {
        self.quack_alias = self
            .active_target()
            .map(|t| t.attached_alias.clone().unwrap_or_else(|| t.label.clone()));
    }

    /// Opens the modal, loading declarative `[quack_auth]` config from `duckpack.toml`
    /// and discovering all attached Quack & DuckLake-on-Quack targets.
    #[allow(dead_code)]
    pub(crate) fn open(
        &mut self,
        conn: &Connection,
        project_dir: &Path,
        preferred_quack_alias: Option<&str>,
    ) {
        self.open_with_env(conn, project_dir, preferred_quack_alias, None);
    }

    pub(crate) fn open_with_env(
        &mut self,
        conn: &Connection,
        project_dir: &Path,
        preferred_quack_alias: Option<&str>,
        env_profile: Option<&str>,
    ) {
        self.env_profile = env_profile.map(String::from);
        self.targets =
            crate::workspace::discover_quack_control_targets(conn, project_dir, env_profile);

        if let Some(pref) = preferred_quack_alias {
            if let Some(idx) = self.targets.iter().position(|t| {
                t.attached_alias.as_deref() == Some(pref)
                    || t.label.starts_with(&format!("{} ", pref))
            }) {
                self.target_index = idx;
            } else {
                self.target_index = 0;
            }
        } else {
            self.target_index = 0;
        }
        self.sync_active_quack_alias();

        if self.targets.is_empty() {
            let mut exec: &Connection = conn;
            self.quack_alias = crate::diff::find_any_quack_alias(&mut exec, preferred_quack_alias);
        }

        if let Some(cfg) = load_quack_auth_config(project_dir) {
            self.config = cfg;
            self.has_config_on_disk = true;
        } else {
            self.config = QuackAuthConfig {
                enabled: true,
                ..Default::default()
            };
            self.has_config_on_disk = false;
        }

        self.is_open = true;
        self.adding_user = false;
        self.pending_break_glass = None;
        self.access_denied = false;
        self.current_sid = None;
        self.current_username = None;
        self.status_msg = None;
        self.error_msg = None;
        self.user_index = self
            .user_index
            .min(self.config.users.len().saturating_sub(1));

        if self.quack_alias.is_some() || !self.targets.is_empty() {
            self.active_tab = QuackAuthTab::LiveSessions;
            self.refresh_sessions_for(conn, project_dir);
        } else {
            self.active_tab = QuackAuthTab::Users;
            self.sessions.clear();
            self.live_server_users.clear();
            self.live_rbac_enabled = None;
            self.session_index = 0;
            self.status_msg = Some(
                "No live Quack server attached — manage declarative users & export scripts offline."
                    .to_string(),
            );
        }
    }

    pub(crate) fn refresh_sessions_for(&mut self, conn: &Connection, project_dir: &Path) {
        if self.active_target().is_none() && self.quack_alias.is_none() {
            self.sessions.clear();
            self.live_server_users.clear();
            self.live_rbac_enabled = None;
            self.current_sid = None;
            self.current_username = None;
            self.access_denied = false;
            self.session_index = 0;
            return;
        }
        let cfg = self.config.clone();
        match self.execute_on_active_quack(conn, project_dir, |c, qa| {
            let sessions = load_live_quack_sessions(c, qa, Some(&cfg))?;
            let server_users = load_live_quack_users(c, qa).unwrap_or_default();
            let rbac_enabled = load_live_quack_rbac_enabled(c, qa);
            let (my_sid, my_user) = load_current_quack_identity(c, qa);
            Ok((sessions, server_users, rbac_enabled, my_sid, my_user))
        }) {
            Ok((list, server_users, rbac_enabled, my_sid, my_user)) => {
                self.sessions = list;
                self.live_server_users = server_users;
                self.live_rbac_enabled = rbac_enabled;
                self.current_sid = my_sid;
                self.current_username = my_user;
                self.access_denied = false;
                self.session_index = self
                    .session_index
                    .min(self.sessions.len().saturating_sub(1));
                if self.is_remote_audit_mode() {
                    self.user_index = self
                        .user_index
                        .min(self.live_server_users.len().saturating_sub(1));
                }
                self.error_msg = None;
            }
            Err(e) => {
                let full_err = format!("{:#}", e);
                let lower = full_err.to_ascii_lowercase();
                let is_denied = lower.contains("authorization failed")
                    || lower.contains("authentication failed");
                self.sessions.clear();
                self.live_server_users.clear();
                self.live_rbac_enabled = None;
                self.current_sid = None;
                self.current_username = None;
                self.access_denied = is_denied;
                if is_denied {
                    self.error_msg = Some(
                        "Admin credentials required — server denied RBAC inspection (Authorization failed)."
                            .to_string(),
                    );
                } else {
                    self.error_msg = Some(format!("Session query failed: {}", e));
                }
            }
        }
    }

    pub(crate) fn paste_text(&mut self, text: &str) {
        if !self.adding_user {
            return;
        }
        let clean = text.replace(['\r', '\n'], "");
        let target = match self.add_field {
            0 => &mut self.new_username,
            2 => &mut self.new_token_env,
            3 => &mut self.new_client_id,
            _ => return,
        };
        for c in clean.chars() {
            insert_char_at_cursor(target, &mut self.cursor_pos, c);
        }
    }

    fn active_add_field_len(&self) -> usize {
        match self.add_field {
            0 => self.new_username.chars().count(),
            2 => self.new_token_env.chars().count(),
            3 => self.new_client_id.chars().count(),
            _ => 0,
        }
    }

    fn autofill_derived_user_fields(&mut self) {
        let uname = self.new_username.trim();
        if !uname.is_empty() && self.new_token_env.trim().is_empty() {
            let env_key = format!(
                "QUACK_USER_{}_TOKEN",
                uname
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() {
                        c.to_ascii_uppercase()
                    } else {
                        '_'
                    })
                    .collect::<String>()
            );
            self.new_token_env = env_key;
        }
    }

    fn submit_new_user(&mut self, project_dir: &Path) -> Option<String> {
        let username = self.new_username.trim().to_string();
        if let Err(e) = validate_safe_username(&username) {
            self.error_msg = Some(format!("{}", e));
            return None;
        }
        if self
            .config
            .users
            .iter()
            .any(|u| u.username.eq_ignore_ascii_case(&username))
        {
            self.error_msg = Some(format!("User '{}' already exists.", username));
            return None;
        }

        self.autofill_derived_user_fields();
        let token_env = self.new_token_env.trim().to_string();
        if token_env.is_empty() {
            self.error_msg = Some("Token env variable name cannot be empty.".to_string());
            return None;
        }
        let client_id = {
            let trimmed = self.new_client_id.trim();
            if trimmed.is_empty() {
                None
            } else if !self.config.allow_insecure_client_id {
                self.error_msg = Some(
                    "Leave Client ID blank (binds securely to secret token). Set allow_insecure_client_id = true in duckpack.toml only for legacy setups."
                        .to_string(),
                );
                return None;
            } else if trimmed.chars().count() < 4 {
                self.error_msg = Some(
                    "Client ID must be at least 4 characters (required by DuckDB Quack)."
                        .to_string(),
                );
                return None;
            } else {
                Some(trimmed.to_string())
            }
        };

        let generated_token = if resolve_user_token(project_dir, &token_env).is_none() {
            match generate_and_save_user_token(project_dir, &token_env) {
                Ok(_) => true,
                Err(e) => {
                    self.error_msg = Some(format!("Failed generating token in .env: {}", e));
                    return None;
                }
            }
        } else {
            false
        };

        self.config.users.push(QuackAuthUser {
            username: username.clone(),
            role: self.new_role,
            token_env: token_env.clone(),
            client_id,
            enabled: true,
        });
        self.config.enabled = true;

        if let Err(e) = save_quack_auth_config(project_dir, &self.config) {
            self.config.users.pop();
            self.error_msg = Some(format!("Failed saving duckpack.toml: {}", e));
            return None;
        }

        self.has_config_on_disk = true;
        self.user_index = self.config.users.len().saturating_sub(1);
        self.adding_user = false;
        self.error_msg = None;
        let msg = if generated_token {
            format!(
                "Added '{}' ({}) & generated 256-bit token in .env ({}). Press 'd' to deploy.",
                username,
                self.new_role.as_str(),
                token_env
            )
        } else {
            format!(
                "Added '{}' ({}) using existing {}. Press 'd' to deploy.",
                username,
                self.new_role.as_str(),
                token_env
            )
        };
        self.status_msg = Some(msg.clone());
        Some(msg)
    }

    fn handle_add_user_key(&mut self, key: KeyEvent, project_dir: &Path) -> Option<String> {
        match key.code {
            KeyCode::Esc => {
                self.adding_user = false;
                self.error_msg = None;
                None
            }
            KeyCode::Tab | KeyCode::Down => {
                if self.add_field == 0 {
                    self.autofill_derived_user_fields();
                }
                self.add_field = (self.add_field + 1) % 4;
                self.cursor_pos = self.active_add_field_len();
                None
            }
            KeyCode::BackTab | KeyCode::Up => {
                if self.add_field == 0 {
                    self.autofill_derived_user_fields();
                }
                self.add_field = (self.add_field + 3) % 4;
                self.cursor_pos = self.active_add_field_len();
                None
            }
            KeyCode::Enter => self.submit_new_user(project_dir),
            KeyCode::Left => {
                if self.add_field == 1 {
                    self.new_role = self.new_role.prev();
                } else {
                    self.cursor_pos = self.cursor_pos.saturating_sub(1);
                }
                None
            }
            KeyCode::Right => {
                if self.add_field == 1 {
                    self.new_role = self.new_role.next();
                } else {
                    let len = self.active_add_field_len();
                    self.cursor_pos = (self.cursor_pos + 1).min(len);
                }
                None
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
                None
            }
            KeyCode::End => {
                self.cursor_pos = self.active_add_field_len();
                None
            }
            KeyCode::Backspace => {
                let target = match self.add_field {
                    0 => &mut self.new_username,
                    2 => &mut self.new_token_env,
                    3 => &mut self.new_client_id,
                    _ => return None,
                };
                delete_char_before_cursor(target, &mut self.cursor_pos);
                None
            }
            KeyCode::Delete => {
                let target = match self.add_field {
                    0 => &mut self.new_username,
                    2 => &mut self.new_token_env,
                    3 => &mut self.new_client_id,
                    _ => return None,
                };
                delete_char_at_cursor(target, self.cursor_pos);
                None
            }
            KeyCode::Char(' ') if self.add_field == 1 => {
                self.new_role = self.new_role.next();
                None
            }
            KeyCode::Char(c) => {
                let target = match self.add_field {
                    0 => &mut self.new_username,
                    2 => &mut self.new_token_env,
                    3 => &mut self.new_client_id,
                    _ => return None,
                };
                insert_char_at_cursor(target, &mut self.cursor_pos, c);
                None
            }
            _ => None,
        }
    }

    fn handle_break_glass_confirm_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        let Some(action) = self.pending_break_glass.clone() else {
            return None;
        };
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.pending_break_glass = None;
                let res = match action {
                    PendingBreakGlass::SetEnabled { username, enabled } => self
                        .execute_on_active_quack(conn, project_dir, |c, qa| {
                            set_live_quack_user_enabled(c, qa, &username, enabled)
                        })
                        .map(|_| {
                            if enabled {
                                format!(
                                    "BREAK-GLASS: Re-enabled '{}' live on server (ephemeral — update central RBAC to persist).",
                                    username
                                )
                            } else {
                                format!(
                                    "BREAK-GLASS: Disabled '{}' & revoked active sessions (ephemeral — update central RBAC to persist).",
                                    username
                                )
                            }
                        }),
                    PendingBreakGlass::SetRole {
                        username,
                        from_role,
                        to_role,
                    } => self
                        .execute_on_active_quack(conn, project_dir, |c, qa| {
                            set_live_quack_user_role(c, qa, &username, to_role)
                        })
                        .map(|_| {
                            format!(
                                "BREAK-GLASS: Changed '{}' live role {} -> {} (ephemeral — update central RBAC to persist).",
                                username,
                                from_role,
                                to_role.as_str()
                            )
                        }),
                };
                match res {
                    Ok(msg) => {
                        self.refresh_sessions_for(conn, project_dir);
                        self.error_msg = None;
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Break-glass action failed: {}", e));
                        None
                    }
                }
            }
            _ => {
                self.pending_break_glass = None;
                self.error_msg = None;
                let msg = "Cancelled break-glass action.".to_string();
                self.status_msg = Some(msg.clone());
                Some(msg)
            }
        }
    }

    /// Handles a key event while the Quack RBAC & Sessions modal is open.
    /// Returns `Some(status_message)` when a notable action completes.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        if self.adding_user {
            return self.handle_add_user_key(key, project_dir);
        }
        if self.pending_break_glass.is_some() {
            return self.handle_break_glass_confirm_key(key, conn, project_dir);
        }

        match key.code {
            KeyCode::Esc => {
                self.is_open = false;
                self.pending_break_glass = None;
                return Some("Closed Quack RBAC & Session Monitor.".to_string());
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.pending_break_glass = None;
                self.active_tab = match self.active_tab {
                    QuackAuthTab::LiveSessions => QuackAuthTab::Users,
                    QuackAuthTab::Users => {
                        self.refresh_sessions_for(conn, project_dir);
                        QuackAuthTab::LiveSessions
                    }
                };
                return None;
            }
            KeyCode::Char('[') => {
                if self.targets.len() > 1 {
                    self.pending_break_glass = None;
                    self.target_index = if self.target_index == 0 {
                        self.targets.len() - 1
                    } else {
                        self.target_index - 1
                    };
                    self.sync_active_quack_alias();
                    self.refresh_sessions_for(conn, project_dir);
                    if let Some(t) = self.active_target() {
                        let msg = format!("Switched Quack monitor target -> {}", t.label);
                        self.status_msg = Some(msg.clone());
                        return Some(msg);
                    }
                }
                return None;
            }
            KeyCode::Char(']') => {
                if self.targets.len() > 1 {
                    self.pending_break_glass = None;
                    self.target_index = (self.target_index + 1) % self.targets.len();
                    self.sync_active_quack_alias();
                    self.refresh_sessions_for(conn, project_dir);
                    if let Some(t) = self.active_target() {
                        let msg = format!("Switched Quack monitor target -> {}", t.label);
                        self.status_msg = Some(msg.clone());
                        return Some(msg);
                    }
                }
                return None;
            }
            KeyCode::Char('1') => {
                self.active_tab = QuackAuthTab::LiveSessions;
                self.refresh_sessions_for(conn, project_dir);
                return None;
            }
            KeyCode::Char('2') => {
                self.active_tab = QuackAuthTab::Users;
                return None;
            }
            _ => {}
        }

        match self.active_tab {
            QuackAuthTab::LiveSessions => self.handle_sessions_key(key, conn, project_dir),
            QuackAuthTab::Users => self.handle_users_key(key, conn, project_dir),
        }
    }

    fn execute_on_active_quack<T, F>(
        &self,
        conn: &Connection,
        project_dir: &Path,
        op: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce(&Connection, &str) -> anyhow::Result<T>,
    {
        if let Some(target) = self.active_target() {
            if target.attached_alias.is_some() || target.quack_endpoint.is_some() {
                return target.with_connection(conn, project_dir, self.env_profile.as_deref(), op);
            }
        }
        if let Some(ref qa) = self.quack_alias {
            return op(conn, qa);
        }
        anyhow::bail!("No Quack server attached.")
    }

    fn handle_sessions_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.session_index = self.session_index.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.sessions.is_empty() {
                    self.session_index = (self.session_index + 1).min(self.sessions.len() - 1);
                }
                None
            }
            KeyCode::Char('u') | KeyCode::Char('U') | KeyCode::F(5) => {
                self.refresh_sessions_for(conn, project_dir);
                if self.error_msg.is_none() {
                    let msg = format!(
                        "Refreshed {} live Quack session(s).",
                        self.sessions.len()
                    );
                    self.status_msg = Some(msg.clone());
                    Some(msg)
                } else {
                    None
                }
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                let Some(sess) = self.sessions.get(self.session_index).cloned() else {
                    return None;
                };
                if self.is_current_sid(&sess.connection_id) {
                    self.error_msg = Some(
                        "Cannot cancel your own active control session.".to_string(),
                    );
                    return None;
                }
                match self.execute_on_active_quack(conn, project_dir, |c, qa| {
                    cancel_quack_session_query(c, qa, &sess.server_id, &sess.connection_id)
                }) {
                    Ok(()) => {
                        self.refresh_sessions_for(conn, project_dir);
                        let msg = format!(
                            "Cancelled active query for session {} ({}).",
                            short_id(&sess.connection_id),
                            sess.username
                        );
                        self.error_msg = None;
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Cancel failed: {}", e));
                        None
                    }
                }
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                let Some(sess) = self.sessions.get(self.session_index).cloned() else {
                    return None;
                };
                if !sess.revoked && self.is_current_sid(&sess.connection_id) {
                    self.error_msg = Some(
                        "Cannot revoke your own active session — prevents self-lockout."
                            .to_string(),
                    );
                    return None;
                }
                let new_revoked = !sess.revoked;
                let preserved_role = if sess.role.eq_ignore_ascii_case("none") {
                    self.config.default_role.as_str()
                } else {
                    sess.role.as_str()
                };
                match self.execute_on_active_quack(conn, project_dir, |c, qa| {
                    set_quack_session_override(
                        c,
                        qa,
                        &sess.connection_id,
                        &sess.username,
                        preserved_role,
                        new_revoked,
                    )
                }) {
                    Ok(()) => {
                        self.refresh_sessions_for(conn, project_dir);
                        let action = if new_revoked { "REVOKED" } else { "RESTORED" };
                        let msg = format!(
                            "{} session {} ({}).",
                            action,
                            short_id(&sess.connection_id),
                            sess.username
                        );
                        self.error_msg = None;
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Session revoke toggle failed: {}", e));
                        None
                    }
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                let Some(sess) = self.sessions.get(self.session_index).cloned() else {
                    return None;
                };
                if self.is_current_sid(&sess.connection_id) {
                    self.error_msg = Some(
                        "Cannot change role of your own active session — prevents self-lockout."
                            .to_string(),
                    );
                    return None;
                }
                let next_role = QuackRole::from_str(&sess.role).next();
                match self.execute_on_active_quack(conn, project_dir, |c, qa| {
                    set_quack_session_override(
                        c,
                        qa,
                        &sess.connection_id,
                        &sess.username,
                        next_role.as_str(),
                        false,
                    )
                }) {
                    Ok(()) => {
                        self.refresh_sessions_for(conn, project_dir);
                        let msg = format!(
                            "Set session {} ({}) role override -> {}.",
                            short_id(&sess.connection_id),
                            sess.username,
                            next_role.as_str()
                        );
                        self.error_msg = None;
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Role override failed: {}", e));
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn handle_users_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        if self.is_admin_locked_view() {
            if matches!(key.code, KeyCode::Char('u') | KeyCode::Char('U') | KeyCode::F(5)) {
                self.refresh_sessions_for(conn, project_dir);
            }
            return None;
        }
        if self.is_remote_audit_mode() {
            return self.handle_remote_audit_users_key(key, conn, project_dir);
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.user_index = self.user_index.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.config.users.is_empty() {
                    self.user_index = (self.user_index + 1).min(self.config.users.len() - 1);
                }
                None
            }
            KeyCode::Char('a') | KeyCode::Char('n') => {
                self.adding_user = true;
                self.new_username.clear();
                self.new_role = QuackRole::Reader;
                self.new_token_env.clear();
                self.new_client_id.clear();
                self.add_field = 0;
                self.cursor_pos = 0;
                self.error_msg = None;
                None
            }
            KeyCode::Char(' ') => {
                if let Some(u) = self.config.users.get(self.user_index) {
                    let mut candidate = self.config.clone();
                    candidate.users[self.user_index].enabled = !u.enabled;
                    if candidate.enabled {
                        if let Err(e) = validate_quack_auth_config(&candidate, project_dir) {
                            self.error_msg = Some(format!("Blocked: {}", e));
                            return None;
                        }
                    }
                    if let Err(e) = save_quack_auth_config(project_dir, &candidate) {
                        self.error_msg = Some(format!("Failed saving duckpack.toml: {}", e));
                        return None;
                    }
                    let uname = candidate.users[self.user_index].username.clone();
                    let state_lbl = if candidate.users[self.user_index].enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    self.config = candidate;
                    self.has_config_on_disk = true;
                    self.error_msg = None;
                    let msg = format!(
                        "User '{}' {} in duckpack.toml. Press 'd' to deploy.",
                        uname, state_lbl
                    );
                    self.status_msg = Some(msg.clone());
                    Some(msg)
                } else {
                    None
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if let Some(u) = self.config.users.get(self.user_index) {
                    let mut candidate = self.config.clone();
                    let next_role = u.role.next();
                    candidate.users[self.user_index].role = next_role;
                    if candidate.enabled {
                        if let Err(e) = validate_quack_auth_config(&candidate, project_dir) {
                            self.error_msg = Some(format!("Blocked: {}", e));
                            return None;
                        }
                    }
                    if let Err(e) = save_quack_auth_config(project_dir, &candidate) {
                        self.error_msg = Some(format!("Failed saving duckpack.toml: {}", e));
                        return None;
                    }
                    let uname = candidate.users[self.user_index].username.clone();
                    self.config = candidate;
                    self.has_config_on_disk = true;
                    self.error_msg = None;
                    let msg = format!(
                        "Changed '{}' role -> {} in duckpack.toml. Press 'd' to deploy.",
                        uname,
                        next_role.as_str()
                    );
                    self.status_msg = Some(msg.clone());
                    Some(msg)
                } else {
                    None
                }
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if let Some(u) = self.config.users.get(self.user_index).cloned() {
                    match generate_and_save_user_token(project_dir, &u.token_env) {
                        Ok(_) => {
                            self.error_msg = None;
                            let msg = format!(
                                "Rotated 256-bit token for '{}' in .env ({}). Press 'd' to deploy.",
                                u.username, u.token_env
                            );
                            self.status_msg = Some(msg.clone());
                            Some(msg)
                        }
                        Err(e) => {
                            self.error_msg = Some(format!("Token rotation failed: {}", e));
                            None
                        }
                    }
                } else {
                    None
                }
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if self.user_index < self.config.users.len() {
                    let mut candidate = self.config.clone();
                    let removed = candidate.users.remove(self.user_index);
                    if candidate.enabled {
                        if let Err(e) = validate_quack_auth_config(&candidate, project_dir) {
                            self.error_msg = Some(format!("Blocked: {}", e));
                            return None;
                        }
                    }
                    if let Err(e) = save_quack_auth_config(project_dir, &candidate) {
                        self.error_msg = Some(format!("Failed saving duckpack.toml: {}", e));
                        return None;
                    }
                    self.config = candidate;
                    self.has_config_on_disk = true;
                    self.user_index = self
                        .user_index
                        .min(self.config.users.len().saturating_sub(1));
                    self.error_msg = None;
                    let msg = format!(
                        "Removed user '{}' from duckpack.toml. Press 'd' to deploy.",
                        removed.username
                    );
                    self.status_msg = Some(msg.clone());
                    Some(msg)
                } else {
                    None
                }
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                let mut candidate = self.config.clone();
                candidate.enabled = !candidate.enabled;
                if candidate.enabled {
                    if let Err(e) = validate_quack_auth_config(&candidate, project_dir) {
                        self.error_msg = Some(format!("Cannot enable RBAC: {}", e));
                        return None;
                    }
                }
                if let Err(e) = save_quack_auth_config(project_dir, &candidate) {
                    self.error_msg = Some(format!("Failed saving duckpack.toml: {}", e));
                    return None;
                }
                self.config = candidate;
                self.has_config_on_disk = true;
                self.error_msg = None;
                let lbl = if self.config.enabled {
                    "ENABLED"
                } else {
                    "DISABLED"
                };
                let msg = format!(
                    "Quack RBAC policy {} in duckpack.toml. Press 'd' to sync server.",
                    lbl
                );
                self.status_msg = Some(msg.clone());
                Some(msg)
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                let target_lbl = self
                    .active_target()
                    .map(|t| t.label.clone())
                    .or_else(|| self.quack_alias.clone())
                    .unwrap_or_else(|| "local".to_string());
                let deploy_res = if self
                    .active_target()
                    .map(|t| t.attached_alias.is_some() || t.quack_endpoint.is_some())
                    .unwrap_or(false)
                    || self.quack_alias.is_some()
                {
                    self.execute_on_active_quack(conn, project_dir, |c, qa| {
                        let mut exec: &Connection = c;
                        deploy_quack_rbac_now(&mut exec, Some(qa), project_dir)
                    })
                    .map(|n| (n, target_lbl))
                } else {
                    let mut exec: &Connection = conn;
                    deploy_quack_rbac_now(&mut exec, None, project_dir).map(|n| (n, target_lbl))
                };

                match deploy_res {
                    Ok((count, target_lbl)) => {
                        self.refresh_sessions_for(conn, project_dir);
                        self.error_msg = None;
                        let msg = format!(
                            "✓ Synced {} Quack RBAC user(s) & activated hooks on '{}'.",
                            count, target_lbl
                        );
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Deploy failed: {}", e));
                        None
                    }
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                match export_quack_rbac_post_start_script(project_dir, self.env_profile.as_deref()) {
                    Ok(sh_path) => {
                        let sql_note = match export_quack_rbac_bootstrap_script(
                            &self.config,
                            project_dir,
                        ) {
                            Ok(sql_path) => format!(
                                " & {}",
                                sql_path
                                    .strip_prefix(project_dir)
                                    .unwrap_or(&sql_path)
                                    .display()
                            ),
                            Err(_) => String::new(),
                        };
                        self.error_msg = None;
                        let rel_sh = sh_path
                            .strip_prefix(project_dir)
                            .unwrap_or(&sh_path)
                            .display();
                        let msg = format!(
                            "Exported post-start hook {}{} (chmod 0755).",
                            rel_sh, sql_note
                        );
                        self.status_msg = Some(msg.clone());
                        Some(msg)
                    }
                    Err(e) => {
                        self.error_msg = Some(format!("Export failed: {}", e));
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn handle_remote_audit_users_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.user_index = self.user_index.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.live_server_users.is_empty() {
                    self.user_index =
                        (self.user_index + 1).min(self.live_server_users.len() - 1);
                }
                None
            }
            KeyCode::Char('u') | KeyCode::Char('U') | KeyCode::F(5) => {
                self.refresh_sessions_for(conn, project_dir);
                if self.error_msg.is_none() {
                    let msg = format!(
                        "Refreshed {} live server user(s).",
                        self.live_server_users.len()
                    );
                    self.status_msg = Some(msg.clone());
                    Some(msg)
                } else {
                    None
                }
            }
            KeyCode::Char(' ') => {
                let Some(u) = self.live_server_users.get(self.user_index).cloned() else {
                    return None;
                };
                if u.username.eq_ignore_ascii_case("__duckpack_master") {
                    self.error_msg = Some(
                        "Cannot disable '__duckpack_master' (protected server break-glass admin)."
                            .to_string(),
                    );
                    return None;
                }
                if u.enabled && self.is_current_username(&u.username) {
                    self.error_msg = Some(format!(
                        "Cannot disable your own active login ('{}') — prevents self-lockout.",
                        u.username
                    ));
                    return None;
                }
                self.error_msg = None;
                self.pending_break_glass = Some(PendingBreakGlass::SetEnabled {
                    username: u.username,
                    enabled: !u.enabled,
                });
                None
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                let Some(u) = self.live_server_users.get(self.user_index).cloned() else {
                    return None;
                };
                if u.username.eq_ignore_ascii_case("__duckpack_master") {
                    self.error_msg = Some(
                        "Cannot change role of '__duckpack_master' (protected server break-glass admin)."
                            .to_string(),
                    );
                    return None;
                }
                if self.is_current_username(&u.username) {
                    self.error_msg = Some(format!(
                        "Cannot change role of your own active login ('{}') — prevents self-lockout.",
                        u.username
                    ));
                    return None;
                }
                let next_role = QuackRole::from_str(&u.role).next();
                self.error_msg = None;
                self.pending_break_glass = Some(PendingBreakGlass::SetRole {
                    username: u.username,
                    from_role: u.role,
                    to_role: next_role,
                });
                None
            }
            KeyCode::Char('a')
            | KeyCode::Char('n')
            | KeyCode::Char('t')
            | KeyCode::Char('T')
            | KeyCode::Delete
            | KeyCode::Backspace
            | KeyCode::Char('g')
            | KeyCode::Char('G')
            | KeyCode::Char('d')
            | KeyCode::Char('D')
            | KeyCode::Char('e')
            | KeyCode::Char('E') => {
                self.status_msg = None;
                self.error_msg = Some(
                    "Read-only audit view: manage users/tokens in your central RBAC repo (break-glass Space / r only)."
                        .to_string(),
                );
                None
            }
            _ => None,
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = ((f.area().width * 9) / 10)
            .clamp(60, 122)
            .min(f.area().width.saturating_sub(2));
        let popup_height = ((f.area().height * 85) / 100)
            .clamp(18, 34)
            .min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(Clear, popup_area);

        let target_badge = if self.targets.len() > 1 {
            let lbl = self
                .active_target()
                .map(|t| t.label.as_str())
                .unwrap_or("unknown");
            format!(
                "◄ [{}/{}] {} ► ([/]: switch)",
                self.target_index + 1,
                self.targets.len(),
                lbl
            )
        } else if let Some(t) = self.active_target() {
            t.label.clone()
        } else if let Some(ref qa) = self.quack_alias {
            format!("Quack: {}", qa)
        } else {
            "Offline (no Quack attached)".to_string()
        };
        let rbac_badge = if self.is_admin_locked_view() {
            "ADMIN ONLY"
        } else if self.is_remote_audit_mode() {
            match self.live_rbac_enabled {
                Some(true) => "RBAC: ON (remote)",
                Some(false) => "RBAC: OFF (remote)",
                None => "RBAC: REMOTE",
            }
        } else if self.config.enabled {
            "RBAC: ON"
        } else {
            "RBAC: OFF"
        };
        let title = format!(
            " 🛡️ Quack RBAC & Live Sessions (Ctrl+U) — {} | {} ",
            target_badge, rbac_badge
        );

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        f.render_widget(outer_block.clone(), popup_area);

        let inner = outer_block.inner(popup_area);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Tab bar
                Constraint::Min(8),    // Main content
                Constraint::Length(2), // Status / error banner
                Constraint::Length(2), // Controls footer
            ])
            .split(inner);

        // Tab header
        let tab1_style = if self.active_tab == QuackAuthTab::LiveSessions {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let tab2_style = if self.active_tab == QuackAuthTab::Users {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let tab_line = Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(" [1] Live Sessions ({}) ", self.sessions.len()),
                tab1_style,
            ),
            Span::raw("  "),
            Span::styled(
                if self.config.users.is_empty() && !self.live_server_users.is_empty() {
                    format!(" [2] Server Users ({}) ", self.live_server_users.len())
                } else {
                    format!(" [2] Declarative Users ({}) ", self.config.users.len())
                },
                tab2_style,
            ),
            Span::styled(
                "   (Tab to switch)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(tab_line), layout[0]);

        // Main body
        match self.active_tab {
            QuackAuthTab::LiveSessions => self.render_sessions_tab(f, layout[1]),
            QuackAuthTab::Users => self.render_users_tab(f, layout[1]),
        }

        // Status / Error / Break-Glass Confirmation banner
        if let Some(ref action) = self.pending_break_glass {
            let prompt = match action {
                PendingBreakGlass::SetEnabled { username, enabled } => {
                    if *enabled {
                        format!(
                            " ⚠️ BREAK-GLASS: Re-enable '{}' live on server? Ephemeral until next RBAC sync. Confirm [y/N]",
                            username
                        )
                    } else {
                        format!(
                            " ⚠️ BREAK-GLASS: Disable '{}' & revoke active sessions now? Ephemeral until next RBAC sync. Confirm [y/N]",
                            username
                        )
                    }
                }
                PendingBreakGlass::SetRole {
                    username,
                    from_role,
                    to_role,
                } => format!(
                    " ⚠️ BREAK-GLASS: Override '{}' live role {} -> {}? Ephemeral until next RBAC sync. Confirm [y/N]",
                    username,
                    from_role,
                    to_role.as_str()
                ),
            };
            let p = Paragraph::new(prompt)
                .style(
                    Style::default()
                        .bg(Color::Yellow)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(p, layout[2]);
        } else if let Some(ref err) = self.error_msg {
            let p = Paragraph::new(format!(" ⚠️ {}", err))
                .style(
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                )
                .wrap(Wrap { trim: true });
            f.render_widget(p, layout[2]);
        } else if let Some(ref msg) = self.status_msg {
            let p = Paragraph::new(format!(" ✓ {}", msg))
                .style(Style::default().fg(Color::Green))
                .wrap(Wrap { trim: true });
            f.render_widget(p, layout[2]);
        } else {
            let hint = if self.active_tab == QuackAuthTab::LiveSessions {
                " Tip: Use 'x' to revoke/restore a session immediately, 'r' to override role, 'c' to cancel query."
            } else if self.is_remote_audit_mode() {
                " Read-only audit view (managed via central RBAC). Break-glass: Space = Disable/Enable, r = Temp Role (resets on next sync)."
            } else {
                " Tip: Changes save to duckpack.toml & .env (0600). Press 'd' to deploy/sync, 'e' to export post-start hook."
            };
            f.render_widget(
                Paragraph::new(hint)
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                layout[2],
            );
        }

        // Footer keybindings
        let footer_text = if self.adding_user {
            " Tab/↑↓: Next Field | ←/→/Space: Role | Enter: Save & Generate Token | Esc: Cancel"
        } else if self.pending_break_glass.is_some() {
            " y / Enter: Confirm Break-Glass Action | n / Esc: Cancel"
        } else if self.is_admin_locked_view()
            || (self.access_denied && self.active_tab == QuackAuthTab::LiveSessions)
        {
            if self.targets.len() > 1 {
                " [/]: Switch Target | u/F5: Retry | Esc: Close"
            } else {
                " u/F5: Retry | Esc: Close"
            }
        } else if self.active_tab == QuackAuthTab::LiveSessions {
            " ↑↓/jk: Select | c: Cancel Query | x: Revoke/Restore | r: Cycle Role | u/F5: Refresh | Tab: Users | Esc: Close"
        } else if self.is_remote_audit_mode() {
            " ↑↓/jk: Select | Break-Glass: Space (Disable/Enable) • r (Temp Role) | u/F5: Refresh | Tab: Sessions | Esc: Close"
        } else {
            " a: Add User | Space: Enable/Disable | r: Role | t: Rotate Token | Del: Remove | g: Toggle RBAC | d: Deploy | e: Export Hook"
        };
        let footer = Paragraph::new(footer_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true });
        f.render_widget(footer, layout[3]);
    }

    fn render_admin_locked_notice(&self, f: &mut Frame, area: Rect) {
        let target_lbl = self
            .active_target()
            .map(|t| t.label.as_str())
            .or(self.quack_alias.as_deref())
            .unwrap_or("Quack server");
        let msg = Paragraph::new(format!(
            "\n  🔒 Admin credentials required\n\n  Your current Quack token does not have admin privileges on '{}'.\n  The server enforces this via _duckpack_quack_authorize — non-admin roles (reader / writer)\n  cannot inspect active sessions, view _duckpack_auth.quack_users, or run break-glass actions.\n\n  To manage RBAC, configure an admin token (admin_token_env / token_env) in .env and press u / F5.",
            target_lbl
        ))
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Admin-Only RBAC Control "),
        );
        f.render_widget(msg, area);
    }

    fn render_sessions_tab(&self, f: &mut Frame, area: Rect) {
        if self.access_denied {
            self.render_admin_locked_notice(f, area);
            return;
        }
        if self.quack_alias.is_none() {
            let msg = Paragraph::new(
                "\n  No Quack server is currently attached.\n  Attach a Quack server via Ctrl+A or start with `duckpack explore --db quack:...`,\n  or press [2] / Tab to manage declarative users & export post-start scripts.",
            )
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(msg, area);
            return;
        }
        if self.sessions.is_empty() {
            let msg = Paragraph::new(
                "\n  No active sessions reported by quack_active_connections().\n  Press 'u' or F5 to refresh.",
            )
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(msg, area);
            return;
        }

        let header = Row::new(vec![
            Cell::from("SID"),
            Cell::from("User"),
            Cell::from("Role"),
            Cell::from("State"),
            Cell::from("Started"),
            Cell::from("Active Query"),
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

        let rows: Vec<Row> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let is_sel = idx == self.session_index;
                let role_label = if s.revoked {
                    "REVOKED".to_string()
                } else {
                    s.role.clone()
                };
                let role_style = if s.revoked {
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD)
                } else {
                    role_color_style(&s.role)
                };
                let query_preview = truncate_single_line(&s.query, 44);
                let started_preview = truncate_single_line(&s.query_started_at, 19);
                let row_style = if is_sel {
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let user_label = if self.is_current_sid(&s.connection_id) {
                    format!("{} (you)", s.username)
                } else {
                    s.username.clone()
                };
                Row::new(vec![
                    Cell::from(short_id(&s.connection_id)),
                    Cell::from(user_label),
                    Cell::from(role_label).style(role_style),
                    Cell::from(s.state.clone()),
                    Cell::from(started_preview),
                    Cell::from(query_preview),
                ])
                .style(row_style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(16),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(20),
                Constraint::Min(20),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Active Quack Connections (quack_active_connections()) "),
        );
        f.render_widget(table, area);
    }

    fn render_users_tab(&self, f: &mut Frame, area: Rect) {
        if self.adding_user {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(5), Constraint::Length(9)])
                .split(area);
            self.render_users_table(f, split[0]);
            self.render_add_user_form(f, split[1]);
        } else {
            self.render_users_table(f, area);
        }
    }

    fn render_users_table(&self, f: &mut Frame, area: Rect) {
        if self.is_admin_locked_view() {
            self.render_admin_locked_notice(f, area);
            return;
        }
        if self.is_remote_audit_mode() {
            let header = Row::new(vec![
                Cell::from("Username"),
                Cell::from("Role"),
                Cell::from("Status"),
                Cell::from("Token Hash"),
                Cell::from("Session Hash"),
                Cell::from("Source"),
            ])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
            let rows: Vec<Row> = self
                .live_server_users
                .iter()
                .enumerate()
                .map(|(idx, u)| {
                    let is_sel = idx == self.user_index;
                    let is_master = u.username.eq_ignore_ascii_case("__duckpack_master");
                    let is_self = self.is_current_username(&u.username);
                    let status_cell = if u.enabled {
                        Cell::from("● enabled").style(Style::default().fg(Color::Green))
                    } else {
                        Cell::from("○ disabled").style(Style::default().fg(Color::LightRed))
                    };
                    let tok_cell = if is_master {
                        Cell::from("★ master env").style(Style::default().fg(Color::Yellow))
                    } else if u.has_token_hash {
                        Cell::from("✓ sha256")
                    } else {
                        Cell::from("✗ missing").style(Style::default().fg(Color::LightRed))
                    };
                    let cid_cell = if u.has_client_id_hash {
                        Cell::from("✓ salted")
                    } else {
                        Cell::from("○ pending").style(Style::default().fg(Color::DarkGray))
                    };
                    let src_cell = if is_master {
                        Cell::from("protected break-glass")
                            .style(Style::default().fg(Color::DarkGray))
                    } else if is_self {
                        Cell::from("current login (protected)")
                            .style(Style::default().fg(Color::Cyan))
                    } else {
                        Cell::from("central RBAC (live)")
                            .style(Style::default().fg(Color::DarkGray))
                    };
                    let row_style = if is_sel {
                        Style::default()
                            .bg(Color::DarkGray)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    Row::new(vec![
                        Cell::from(u.username.clone()),
                        Cell::from(u.role.clone()).style(role_color_style(&u.role)),
                        status_cell,
                        tok_cell,
                        cid_cell,
                        src_cell,
                    ])
                    .style(row_style)
                })
                .collect();
            let table = Table::new(
                rows,
                [
                    Constraint::Length(18),
                    Constraint::Length(10),
                    Constraint::Length(12),
                    Constraint::Length(14),
                    Constraint::Length(14),
                    Constraint::Min(22),
                ],
            )
            .header(header)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(" Live Server Users — Read-Only Audit View (_duckpack_auth.quack_users) "),
            );
            f.render_widget(table, area);
            return;
        }
        if self.config.users.is_empty() {
            let msg = Paragraph::new(
                "\n  No declarative Quack users configured in duckpack.toml yet.\n  Press 'a' to add your first Admin user (auto-generates a 256-bit token in .env with 0600 permissions).",
            )
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray))
                    .title(" Declarative Users (duckpack.toml) "),
            );
            f.render_widget(msg, area);
            return;
        }

        let header = Row::new(vec![
            Cell::from("Username"),
            Cell::from("Role"),
            Cell::from("Status"),
            Cell::from("Token Env Var"),
            Cell::from("Client ID"),
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

        let rows: Vec<Row> = self
            .config
            .users
            .iter()
            .enumerate()
            .map(|(idx, u)| {
                let is_sel = idx == self.user_index;
                let status_cell = if u.enabled {
                    Cell::from("● enabled").style(Style::default().fg(Color::Green))
                } else {
                    Cell::from("○ disabled").style(Style::default().fg(Color::LightRed))
                };
                let cid = u.client_id.as_deref().unwrap_or("(token-bound)");
                let row_style = if is_sel {
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Row::new(vec![
                    Cell::from(u.username.clone()),
                    Cell::from(u.role.as_str()).style(role_color_style(u.role.as_str())),
                    status_cell,
                    Cell::from(u.token_env.clone()),
                    Cell::from(cid.to_string()),
                ])
                .style(row_style)
            })
            .collect();

        let table = Table::new(
            rows,
            [
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Min(24),
                Constraint::Length(18),
            ],
        )
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(format!(
                    " Declarative Users (duckpack.toml | default_role={}) ",
                    self.config.default_role.as_str()
                )),
        );
        f.render_widget(table, area);
    }

    fn render_add_user_form(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" ➕ Add Quack User (auto-generates 256-bit token in .env) ")
            .border_style(Style::default().fg(Color::Yellow));
        f.render_widget(block.clone(), area);
        let inner = block.inner(area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .split(inner);

        let label_style = |active: bool| {
            if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            }
        };

        // Field 0: Username
        let mut f0 = vec![Span::styled(" Username  : ", label_style(self.add_field == 0))];
        if self.add_field == 0 {
            f0.extend(render_text_with_cursor(&self.new_username, self.cursor_pos, true, false));
        } else {
            f0.push(Span::raw(self.new_username.clone()));
        }
        f.render_widget(Paragraph::new(Line::from(f0)), rows[0]);

        // Field 1: Role
        let role_line = Line::from(vec![
            Span::styled(" Role      : ", label_style(self.add_field == 1)),
            Span::styled(
                format!("< {} >", self.new_role.as_str()),
                role_color_style(self.new_role.as_str()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  (←/→ or Space to cycle: admin / writer / reader)",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(role_line), rows[1]);

        // Field 2: Token Env Var
        let mut f2 = vec![Span::styled(" Token Env : ", label_style(self.add_field == 2))];
        if self.add_field == 2 {
            f2.extend(render_text_with_cursor(&self.new_token_env, self.cursor_pos, true, false));
        } else if self.new_token_env.is_empty() {
            f2.push(Span::styled(
                "(auto: QUACK_USER_<NAME>_TOKEN)",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            f2.push(Span::raw(self.new_token_env.clone()));
        }
        f.render_widget(Paragraph::new(Line::from(f2)), rows[2]);

        // Field 3: Client ID
        let mut f3 = vec![Span::styled(" Client ID : ", label_style(self.add_field == 3))];
        if self.add_field == 3 {
            f3.extend(render_text_with_cursor(&self.new_client_id, self.cursor_pos, true, false));
        } else if self.new_client_id.is_empty() {
            f3.push(Span::styled(
                "(recommended: blank — binds securely to secret token)",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            f3.push(Span::raw(self.new_client_id.clone()));
        }
        f.render_widget(Paragraph::new(Line::from(f3)), rows[3]);
    }
}

fn role_color_style(role: &str) -> Style {
    match role.trim().to_lowercase().as_str() {
        "admin" => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        "writer" => Style::default().fg(Color::Cyan),
        "reader" => Style::default().fg(Color::Green),
        _ => Style::default().fg(Color::LightRed),
    }
}

fn short_id(id: &str) -> String {
    if id.chars().count() <= 10 {
        id.to_string()
    } else {
        let prefix: String = id.chars().take(10).collect();
        format!("{}…", prefix)
    }
}

fn truncate_single_line(s: &str, max_chars: usize) -> String {
    let one_line: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        let clipped: String = one_line.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", clipped)
    }
}
