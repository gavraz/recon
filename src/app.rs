use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};

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
    pub effort_level: String,
    pub should_quit: bool,
    pub view_mode: ViewMode,
    pub tick: u64,
    pub view_page: usize,
    pub view_zoomed_room: Option<String>, // room name when zoomed in
    pub view_zoom_index: Option<usize>,  // pending zoom request from key press
    pub view_selected_agent: usize,      // selected agent within zoomed room
    prev_sessions: HashMap<String, Session>,
}

impl App {
    pub fn new() -> Self {
        let effort_level = read_effort_level().unwrap_or_else(|| "medium".to_string());
        App {
            sessions: Vec::new(),
            selected: 0,
            effort_level,
            should_quit: false,
            view_mode: ViewMode::Table,
            tick: 0,
            view_page: 0,
            view_zoomed_room: None,
            view_zoom_index: None,
            view_selected_agent: 0,
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

        if self.selected >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
    }

    pub fn advance_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.view_mode {
            ViewMode::Table => self.handle_key_table(key),
            ViewMode::View => self.handle_key_view(key),
        }
    }

    fn handle_key_table(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('v') => self.view_mode = ViewMode::View,
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.sessions.is_empty() {
                    self.selected = (self.selected + 1).min(self.sessions.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Enter => {
                if let Some(session) = self.sessions.get(self.selected) {
                    if let Some(name) = &session.tmux_session {
                        tmux::switch_to_session(name);
                        self.should_quit = true;
                    }
                }
            }
            KeyCode::Char('x') => {
                if let Some(session) = self.sessions.get(self.selected) {
                    if let Some(name) = &session.tmux_session {
                        tmux::kill_session(name);
                        self.refresh();
                    }
                }
            }
            KeyCode::Char('r') => {
                self.refresh();
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
                        if let Some(name) = session.tmux_session.clone() {
                            tmux::switch_to_session(&name);
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
                        if let Ok(name) = tmux::create_session(&default_name, &cwd) {
                            tmux::switch_to_session(&name);
                            self.should_quit = true;
                        }
                    }
                    return;
                }
                _ => {} // fall through to shared keys
            }
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                if self.view_zoomed_room.is_some() {
                    self.view_zoomed_room = None;
                    self.view_selected_agent = 0;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('v') => {
                self.view_zoomed_room = None;
                self.view_selected_agent = 0;
                self.view_mode = ViewMode::Table;
            }
            KeyCode::Char('r') => self.refresh(),
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

    fn zoomed_room_session_indices(&self) -> Vec<usize> {
        let Some(ref room_name) = self.view_zoomed_room else {
            return Vec::new();
        };
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let name = if s.cwd.is_empty() {
                    "unknown".to_string()
                } else {
                    shorten_home(&s.cwd)
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

    pub fn to_json(&self) -> String {
        let sessions: Vec<serde_json::Value> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                serde_json::json!({
                    "index": i + 1,
                    "session_id": s.session_id,
                    "project_name": s.project_name,
                    "branch": s.branch,
                    "cwd": s.cwd,
                    "tmux_session": s.tmux_session,
                    "model": s.model,
                    "model_display": s.model_display(&self.effort_level),
                    "total_input_tokens": s.total_input_tokens,
                    "total_output_tokens": s.total_output_tokens,
                    "context_display": s.token_display(),
                    "token_ratio": s.token_ratio(),
                    "status": s.status.label(),
                    "pid": s.pid,
                    "last_activity": s.last_activity,
                    "started_at": s.started_at,
                })
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "sessions": sessions,
            "effort_level": self.effort_level,
        }))
        .unwrap_or_else(|_| "{}".to_string())
    }
}

fn shorten_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn read_effort_level() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("effortLevel")?.as_str().map(|s| s.to_string())
}
