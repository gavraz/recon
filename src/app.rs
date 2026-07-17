use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Config;
use crate::session::{self, Session};
use crate::tmux;

#[derive(Clone, Copy, PartialEq)]
pub enum ViewMode {
    Table,
    View,
}

pub struct App {
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub should_quit: bool,
    pub view_mode: ViewMode,
    pub tick: u64,
    pub view_page: usize,
    pub view_zoomed_room: Option<String>, // room name when zoomed in
    pub view_zoom_index: Option<usize>,  // pending zoom request from key press
    pub view_selected_agent: usize,      // selected agent within zoomed room
    pub filter_active: bool,              // search input has focus
    pub filter_text: String,              // current search query
    pub filter_cursor: usize,             // cursor position in query
    pub config: Config,                   // table column configuration
    prev_sessions: HashMap<String, Session>,
}

impl App {
    pub fn new() -> Self {
        App {
            sessions: Vec::new(),
            selected: 0,
            should_quit: false,
            view_mode: ViewMode::Table,
            tick: 0,
            view_page: 0,
            view_zoomed_room: None,
            view_zoom_index: None,
            view_selected_agent: 0,
            filter_active: false,
            filter_text: String::new(),
            filter_cursor: 0,
            config: Config::default(),
            prev_sessions: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) {
        let sessions: Vec<Session> = session::discover_sessions(&self.prev_sessions)
            .into_iter()
            .filter(|s| s.tmux_session.is_some())
            .collect();

        self.prev_sessions = sessions
            .iter()
            .map(|s| (s.session_id.clone(), s.clone()))
            .collect();

        self.sessions = sessions;

        let count = self.filtered_indices().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    pub fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        if self.filter_text.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        let query = self.filter_text.to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.project_name.to_lowercase().contains(&query)
                    || s.tmux_session
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                    || s.tmux_window
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_indices().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    /// Resolve filtered index to real session index.
    fn resolve_selected(&self) -> Option<usize> {
        let indices = self.filtered_indices();
        indices.get(self.selected).copied()
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.filter_active {
            self.handle_key_filter(key);
            return;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::Char('i')) {
            self.jump_to_next_input();
            return;
        }
        match self.view_mode {
            ViewMode::Table => self.handle_key_table(key),
            ViewMode::View => self.handle_key_view(key),
        }
    }

    fn jump_to_next_input(&mut self) {
        if let Some(session) = self.sessions.iter().find(|s| s.status == session::SessionStatus::Input) {
            if let Some(target) = &session.pane_target {
                tmux::switch_to_pane(target);
                self.should_quit = true;
            }
        }
    }

    fn handle_key_table(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                if !self.filter_text.is_empty() {
                    self.filter_text.clear();
                    self.selected = 0;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('/') => {
                self.filter_active = true;
                self.filter_text.clear();
                self.filter_cursor = 0;
                self.selected = 0;
            }
            KeyCode::Char('v') => self.view_mode = ViewMode::View,
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.filtered_indices().len();
                if count > 0 {
                    self.selected = (self.selected + 1).min(count - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Enter => {
                if let Some(real_idx) = self.resolve_selected() {
                    if let Some(session) = self.sessions.get(real_idx) {
                        if let Some(target) = &session.pane_target {
                            tmux::switch_to_pane(target);
                            self.should_quit = true;
                        }
                    }
                }
            }
            KeyCode::Char('x') => {
                if let Some(real_idx) = self.resolve_selected() {
                    if let Some(session) = self.sessions.get(real_idx) {
                        if let Some(name) = &session.tmux_session {
                            tmux::kill_session(name);
                            self.refresh();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_key_view(&mut self, key: KeyEvent) {
        // Agent interaction keys (only when zoomed into a room)
        if self.view_zoomed_room.is_some() {
            match key.code {
                KeyCode::Char('l') | KeyCode::Right => {
                    self.view_selected_agent = self.view_selected_agent.saturating_add(1);
                    return;
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    self.view_selected_agent = self.view_selected_agent.saturating_sub(1);
                    return;
                }
                KeyCode::Enter => {
                    if let Some(session) = self.selected_zoomed_session() {
                        if let Some(target) = session.pane_target.clone() {
                            tmux::switch_to_pane(&target);
                            self.should_quit = true;
                        }
                    }
                    return;
                }
                KeyCode::Char('x') => {
                    if let Some(session) = self.selected_zoomed_session() {
                        if let Some(name) = session.tmux_session.clone() {
                            tmux::kill_session(&name);
                            self.refresh();
                        }
                    }
                    return;
                }
                KeyCode::Char('n') => {
                    if let Some(cwd) = self.zoomed_room_cwd() {
                        let default_name = std::path::Path::new(&cwd)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "claude".to_string());
                        if let Ok(name) = tmux::create_session(&default_name, &cwd, None, &[]) {
                            tmux::switch_to_pane(&name);
                            self.should_quit = true;
                        }
                    }
                    return;
                }
                _ => {} // fall through to shared keys
            }
        }

        match key.code {
            KeyCode::Char('/') => {
                self.filter_active = true;
                self.filter_text.clear();
                self.filter_cursor = 0;
                self.selected = 0;
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                if self.view_zoomed_room.is_some() {
                    self.view_zoomed_room = None;
                    self.view_selected_agent = 0;
                } else if !self.filter_text.is_empty() {
                    self.filter_text.clear();
                    self.selected = 0;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('v') => {
                self.view_zoomed_room = None;
                self.view_selected_agent = 0;
                self.view_mode = ViewMode::Table;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.view_page = self.view_page.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.view_page = self.view_page.saturating_sub(1);
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                self.view_zoom_index = Some(idx);
                self.view_selected_agent = 0;
            }
            _ => {}
        }
    }

    fn handle_key_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter_active = false;
                self.filter_text.clear();
                self.filter_cursor = 0;
                self.selected = 0;
            }
            KeyCode::Enter => {
                let indices = self.filtered_indices();
                if indices.len() == 1 {
                    if let Some(session) = self.sessions.get(indices[0]) {
                        if let Some(target) = &session.pane_target {
                            tmux::switch_to_pane(target);
                            self.should_quit = true;
                            return;
                        }
                    }
                }
                self.filter_active = false;
            }
            KeyCode::Backspace => {
                if self.filter_cursor > 0 {
                    let byte_pos = self.filter_text.char_indices()
                        .nth(self.filter_cursor - 1)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    let next_byte = self.filter_text.char_indices()
                        .nth(self.filter_cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.filter_text.len());
                    self.filter_text.replace_range(byte_pos..next_byte, "");
                    self.filter_cursor -= 1;
                    self.clamp_selection();
                }
            }
            KeyCode::Delete => {
                let char_count = self.filter_text.chars().count();
                if self.filter_cursor < char_count {
                    let byte_pos = self.filter_text.char_indices()
                        .nth(self.filter_cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.filter_text.len());
                    let next_byte = self.filter_text.char_indices()
                        .nth(self.filter_cursor + 1)
                        .map(|(i, _)| i)
                        .unwrap_or(self.filter_text.len());
                    self.filter_text.replace_range(byte_pos..next_byte, "");
                    self.clamp_selection();
                }
            }
            KeyCode::Left => {
                if self.filter_cursor > 0 {
                    self.filter_cursor -= 1;
                }
            }
            KeyCode::Right => {
                let char_count = self.filter_text.chars().count();
                if self.filter_cursor < char_count {
                    self.filter_cursor += 1;
                }
            }
            KeyCode::Home => self.filter_cursor = 0,
            KeyCode::End => self.filter_cursor = self.filter_text.chars().count(),
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_cursor = 0;
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_cursor = self.filter_text.chars().count();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter_text.clear();
                self.filter_cursor = 0;
                self.clamp_selection();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self.filtered_indices().len();
                if count > 0 {
                    self.selected = (self.selected + 1).min(count - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Tab | KeyCode::Char('i') => {
                self.jump_to_next_input();
            }
            KeyCode::Char(c) => {
                let byte_pos = self.filter_text.char_indices()
                    .nth(self.filter_cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(self.filter_text.len());
                self.filter_text.insert(byte_pos, c);
                self.filter_cursor += 1;
                self.clamp_selection();
            }
            _ => {}
        }
    }

    fn zoomed_room_session_indices(&self) -> Vec<usize> {
        let Some(ref room_name) = self.view_zoomed_room else {
            return Vec::new();
        };
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let name = if s.project_name.is_empty() {
                    "unknown".to_string()
                } else {
                    s.room_id()
                };
                &name == room_name
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_zoomed_session(&self) -> Option<&Session> {
        let indices = self.zoomed_room_session_indices();
        if indices.is_empty() {
            return None;
        }
        let clamped = self.view_selected_agent.min(indices.len() - 1);
        self.sessions.get(indices[clamped])
    }

    fn zoomed_room_cwd(&self) -> Option<String> {
        self.selected_zoomed_session().map(|s| s.cwd.clone())
    }

    pub fn to_json(&self, tag_filters: &[String]) -> String {
        // Parse tag filters into key:value pairs
        let filters: Vec<(&str, &str)> = tag_filters
            .iter()
            .filter_map(|t| t.split_once(':'))
            .collect();

        let sessions: Vec<serde_json::Value> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                filters.iter().all(|(k, v)| {
                    s.tags.get(*k).map_or(false, |tv| tv == v)
                })
            })
            .map(|(i, s)| {
                serde_json::json!({
                    "index": i + 1,
                    "session_id": s.session_id,
                    "project_name": s.project_name,
                    "branch": s.branch,
                    "cwd": s.cwd,
                    "room_id": s.room_id(),
                    "relative_dir": s.relative_dir,
                    "tmux_session": s.tmux_session,
                    "tmux_window": s.tmux_window,
                    "pane_target": s.pane_target,
                    "model": s.model,
                    "model_display": s.model_display(),
                    "total_input_tokens": s.total_input_tokens,
                    "total_output_tokens": s.total_output_tokens,
                    "context_display": s.token_display(),
                    "token_ratio": s.token_ratio(),
                    "status": s.status.label(),
                    "pid": s.pid,
                    "last_activity": s.last_activity,
                    "started_at": s.started_at,
                    "tags": s.tags,
                })
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "sessions": sessions,
        }))
        .unwrap_or_else(|_| "{}".to_string())
    }
}


