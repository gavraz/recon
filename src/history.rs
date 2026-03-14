use std::collections::HashSet;
use std::fs;
use std::io;
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

use crate::model;

#[derive(Debug, Clone)]
pub struct ResumeEntry {
    pub session_id: String,
    pub cwd: String,
    pub started_at: u64,
    pub model: Option<String>,
    pub tokens: u64,
}

/// Build list of resumable sessions by scanning ~/.claude/sessions/*.json
/// and filtering out sessions that are currently live in tmux.
fn find_resumable_sessions() -> Vec<ResumeEntry> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    // Get currently live session IDs
    let live_ids = get_live_session_ids();

    // Scan all session files
    let sessions_dir = home.join(".claude").join("sessions");
    let entries = match fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut results = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let session_id = match v.get("sessionId").and_then(|s| s.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        // Skip currently live sessions
        if live_ids.contains(&session_id) {
            continue;
        }

        let cwd = v.get("cwd").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let started_at = v.get("startedAt").and_then(|s| s.as_u64()).unwrap_or(0);

        // Skip very old sessions (> 7 days)
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if now_ms.saturating_sub(started_at) > 7 * 24 * 3600 * 1000 {
            continue;
        }

        // Try to get model/tokens from the JSONL
        let (model, tokens) = read_jsonl_summary(&home, &session_id);

        // Skip sessions with no activity
        if tokens == 0 {
            continue;
        }

        results.push(ResumeEntry {
            session_id,
            cwd,
            started_at,
            model,
            tokens,
        });
    }

    // Deduplicate by session_id (keep the one with highest started_at)
    results.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let mut seen = HashSet::new();
    results.retain(|e| seen.insert(e.session_id.clone()));

    // Limit to 10
    results.truncate(10);
    results
}

/// Get session IDs of currently live tmux sessions running claude.
fn get_live_session_ids() -> HashSet<String> {
    let map = crate::session::build_live_session_map_public();
    map.into_keys().collect()
}

/// Read model and total tokens from the JSONL file for a session.
fn read_jsonl_summary(home: &std::path::Path, session_id: &str) -> (Option<String>, u64) {
    let projects_dir = home.join(".claude").join("projects");
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return (None, 0),
    };

    for entry in entries.flatten() {
        let jsonl = entry.path().join(format!("{session_id}.jsonl"));
        if !jsonl.exists() {
            continue;
        }

        let content = match fs::read_to_string(&jsonl) {
            Ok(c) => c,
            Err(_) => return (None, 0),
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
                if model.is_some() && input_tokens > 0 {
                    break;
                }
            }
        }

        return (model, input_tokens + output_tokens);
    }
    (None, 0)
}

/// Interactive TUI picker for resuming a past session.
/// Returns Some((session_id, name)) if the user picks one, None if they cancel.
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

            let header = Row::new(vec![
                Cell::from(" # "),
                Cell::from("Directory"),
                Cell::from("Model"),
                Cell::from("Tokens"),
                Cell::from("Started"),
                Cell::from("Session ID"),
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
                    let started = format_relative_ms(e.started_at);
                    let short_id = &e.session_id[..8.min(e.session_id.len())];

                    let row = Row::new(vec![
                        Cell::from(format!(" {} ", i + 1)),
                        Cell::from(dir),
                        Cell::from(model_display),
                        Cell::from(tokens),
                        Cell::from(started),
                        Cell::from(format!("{short_id}…")),
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
                Constraint::Min(16),
                Constraint::Length(16),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(12),
            ];

            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Resume Session ");

            if entries.is_empty() {
                let msg = Paragraph::new(Line::from(vec![
                    Span::styled("  No resumable sessions found", Style::default().fg(Color::DarkGray)),
                ])).block(block);
                f.render_widget(msg, chunks[0]);
            } else {
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
                        if selected + 1 < entries.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        }
                    }
                    KeyCode::Enter => {
                        let entry = &entries[selected];
                        let name = dir_name(&entry.cwd);
                        result = Some((entry.session_id.clone(), name));
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

fn dir_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn format_relative_ms(started_at_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let diff_secs = now_ms.saturating_sub(started_at_ms) / 1000;

    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86400)
    }
}
