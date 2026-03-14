use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Layout},
    prelude::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Terminal,
};
use serde::{Deserialize, Serialize};

use crate::model;
use crate::session::Session;

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeEntry {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    pub model: Option<String>,
    pub tokens: u64,
    pub saved_at: String,
}

fn history_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".recon").join("history.json"))
}

fn load_history() -> Vec<ResumeEntry> {
    let path = match history_path() {
        Some(p) => p,
        None => return vec![],
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_history(entries: &[ResumeEntry]) {
    let path = match history_path() {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, serde_json::to_string_pretty(entries).unwrap_or_default());
}

/// Called from app.refresh() when a session disappears (was alive, now gone).
/// Saves its info to ~/.recon/history.json for later resume.
pub fn save_exited_session(session: &Session) {
    let mut entries = load_history();

    // Don't add duplicates
    if entries.iter().any(|e| e.session_id == session.session_id) {
        return;
    }

    entries.push(ResumeEntry {
        session_id: session.session_id.clone(),
        name: session.tmux_session.clone().unwrap_or_default(),
        cwd: session.cwd.clone(),
        model: session.model.clone(),
        tokens: session.total_input_tokens + session.total_output_tokens,
        saved_at: chrono::Utc::now().to_rfc3339(),
    });

    // Keep last N
    if entries.len() > MAX_ENTRIES {
        entries = entries.split_off(entries.len() - MAX_ENTRIES);
    }

    save_history(&entries);
}

/// Build the list of resumable sessions by scanning JSONL files (primary, they
/// persist after exit) merged with our saved history (has name/metadata).
/// Filters out any currently live sessions.
fn find_resumable_sessions() -> Vec<ResumeEntry> {
    let live_ids = get_live_session_ids();
    let history = load_history();

    // Scan JSONL files for dead sessions not already in history
    let mut entries: Vec<ResumeEntry> = history
        .into_iter()
        .filter(|e| !live_ids.contains(&e.session_id))
        .collect();

    let known_ids: HashSet<String> = entries.iter().map(|e| e.session_id.clone()).collect();

    if let Some(home) = dirs::home_dir() {
        let projects_dir = home.join(".claude").join("projects");
        if let Ok(dirs) = fs::read_dir(&projects_dir) {
            for dir_entry in dirs.flatten() {
                if let Ok(files) = fs::read_dir(dir_entry.path()) {
                    for file in files.flatten() {
                        let path = file.path();
                        if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                            continue;
                        }
                        let session_id = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();

                        if live_ids.contains(&session_id) || known_ids.contains(&session_id) {
                            continue;
                        }

                        // Skip old files (>7 days)
                        let mtime = path.metadata().ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        if now_ms.saturating_sub(mtime) > 7 * 24 * 3600 * 1000 {
                            continue;
                        }

                        let (model, tokens, _) = read_jsonl_summary(&home, &session_id);
                        if tokens == 0 {
                            continue;
                        }

                        let cwd = decode_project_path(&dir_entry.path());

                        entries.push(ResumeEntry {
                            // No tmux name available from JSONL — show full session ID
                            name: session_id.clone(),
                            session_id,
                            cwd,
                            model,
                            tokens,
                            saved_at: format_epoch_ms(mtime),
                        });
                    }
                }
            }
        }
    }

    // Sort by saved_at descending (most recent first)
    entries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    entries.truncate(MAX_ENTRIES);
    entries
}

fn get_live_session_ids() -> HashSet<String> {
    crate::session::build_live_session_map_public().into_keys().collect()
}

/// Interactive TUI picker for resuming a past session.
pub fn run_resume_picker() -> io::Result<Option<(String, String)>> {
    let entries = find_resumable_sessions();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected = 0usize;
    let result;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
                .split(f.area());

            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Resume Session ");

            if entries.is_empty() {
                let msg = Paragraph::new(Line::from(vec![Span::styled(
                    "  No resumable sessions — sessions appear here when they exit while recon is running",
                    Style::default().fg(Color::DarkGray),
                )]))
                .block(block);
                f.render_widget(msg, chunks[0]);
            } else {
                let header = Row::new(vec![
                    Cell::from(" # "),
                    Cell::from("Session"),
                    Cell::from("Directory"),
                    Cell::from("Model"),
                    Cell::from("Tokens"),
                    Cell::from("Exited"),
                ])
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

                let rows: Vec<Row> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let model_display = e
                            .model
                            .as_deref()
                            .map(|m| model::display_name(m).to_string())
                            .unwrap_or_else(|| "—".to_string());

                        let tokens = format!("{}k", e.tokens / 1000);
                        let dir = dir_name(&e.cwd);
                        let exited = format_relative(&e.saved_at);

                        let row = Row::new(vec![
                            Cell::from(format!(" {} ", i + 1)),
                            Cell::from(e.name.clone()),
                            Cell::from(dir).style(Style::default().fg(Color::DarkGray)),
                            Cell::from(model_display),
                            Cell::from(tokens),
                            Cell::from(exited),
                        ]);

                        if i == selected {
                            row.style(Style::default().bg(Color::DarkGray))
                        } else {
                            row
                        }
                    })
                    .collect();

                let widths = [
                    Constraint::Length(4),
                    Constraint::Length(20),
                    Constraint::Min(16),
                    Constraint::Length(16),
                    Constraint::Length(10),
                    Constraint::Length(14),
                ];

                let table = Table::new(rows, widths).header(header).block(block);
                f.render_widget(table, chunks[0]);
            }

            let footer = Paragraph::new(Line::from(vec![
                Span::styled("j/k", Style::default().fg(Color::Cyan)),
                Span::raw(" navigate  "),
                Span::styled("Enter", Style::default().fg(Color::Cyan)),
                Span::raw(" resume  "),
                Span::styled("q/Esc", Style::default().fg(Color::Cyan)),
                Span::raw(" cancel"),
            ]));
            f.render_widget(footer, chunks[1]);
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        result = None;
                        break;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if !entries.is_empty() && selected + 1 < entries.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        }
                    }
                    KeyCode::Enter => {
                        if entries.is_empty() {
                            result = None;
                        } else {
                            let entry = &entries[selected];
                            result = Some((entry.session_id.clone(), entry.name.clone()));
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(result)
}

/// Decode encoded project dir name back to a path: `-Users-gavra-repos` → `/Users/gavra/repos`
fn decode_project_path(project_dir: &std::path::Path) -> String {
    let name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.starts_with('-') {
        name.replacen('-', "/", 1).replace('-', "/")
    } else {
        name
    }
}

fn format_epoch_ms(ms: u64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Read model and total tokens from a session's JSONL (reads last 50 lines).
fn read_jsonl_summary(home: &std::path::Path, session_id: &str) -> (Option<String>, u64, u64) {
    let projects_dir = home.join(".claude").join("projects");
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return (None, 0, 0),
    };
    for entry in entries.flatten() {
        let jsonl = entry.path().join(format!("{session_id}.jsonl"));
        if !jsonl.exists() {
            continue;
        }
        let mtime_ms = jsonl.metadata().ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let content = match fs::read_to_string(&jsonl) {
            Ok(c) => c,
            Err(_) => return (None, 0, mtime_ms),
        };
        let mut model = None;
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        for line in content.lines().rev().take(50) {
            if line.contains("\"type\":\"assistant\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(msg) = v.get("message") {
                        if model.is_none() {
                            model = msg.get("model").and_then(|m| m.as_str()).map(|s| s.to_string());
                        }
                        if input_tokens == 0 {
                            if let Some(usage) = msg.get("usage") {
                                input_tokens = usage.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0)
                                    + usage.get("cache_creation_input_tokens").and_then(|t| t.as_u64()).unwrap_or(0)
                                    + usage.get("cache_read_input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                                output_tokens = usage.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                            }
                        }
                    }
                }
                if model.is_some() && input_tokens > 0 { break; }
            }
        }
        return (model, input_tokens + output_tokens, mtime_ms);
    }
    (None, 0, 0)
}

fn dir_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn format_relative(ts: &str) -> String {
    use chrono::{DateTime, Utc};
    match ts.parse::<DateTime<Utc>>() {
        Ok(dt) => {
            let diff = Utc::now() - dt;
            if diff.num_minutes() < 1 {
                "just now".to_string()
            } else if diff.num_minutes() < 60 {
                format!("{}m ago", diff.num_minutes())
            } else if diff.num_hours() < 24 {
                format!("{}h ago", diff.num_hours())
            } else {
                format!("{}d ago", diff.num_days())
            }
        }
        Err(_) => ts.to_string(),
    }
}
