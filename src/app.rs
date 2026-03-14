use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};

use crate::session::{self, Session};
use crate::tmux;

pub struct App {
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub effort_level: String,
    pub should_quit: bool,
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

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
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
            KeyCode::Char('r') => {
                self.refresh();
            }
            _ => {}
        }
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
                    "tokens_display": s.token_display(),
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

fn read_effort_level() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("effortLevel")?.as_str().map(|s| s.to_string())
}
