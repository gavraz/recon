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

const MAX_ENTRIES: usize = 10;

#[derive(Debug, Clone)]
pub struct ResumeEntry {
    pub session_id: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub tokens: u64,
    pub last_active: String, // RFC3339
}

/// Build list of resumable sessions by scanning JSONL files and filtering out live ones.
fn find_resumable_sessions() -> Vec<ResumeEntry> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    let live_ids = get_live_session_ids();
    let projects_dir = home.join(".claude").join("projects");
    let dirs = match fs::read_dir(&projects_dir) {
        Ok(d) => d,
        Err(_) => return vec![],
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut entries = Vec::new();

    for dir_entry in dirs.flatten() {
        let files = match fs::read_dir(dir_entry.path()) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let cwd = decode_project_path(&dir_entry.path());

        for file in files.flatten() {
            let path = file.path();
            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }
            // Skip subdirectories (e.g. subagents/)
            if path.parent() != Some(&dir_entry.path()) {
                continue;
            }

            let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            if live_ids.contains(&session_id) {
                continue;
            }

            // Skip old files (>7 days)
            let mtime_ms = file_mtime_ms(&path);
            if now_ms.saturating_sub(mtime_ms) > 7 * 24 * 3600 * 1000 {
                continue;
            }

            let summary = read_jsonl_summary(&path);
            if summary.tokens == 0 {
                continue;
            }

            entries.push(ResumeEntry {
                session_id,
                cwd: cwd.clone(),
                branch: summary.branch,
                model: summary.model,
                tokens: summary.tokens,
                last_active: format_epoch_ms(mtime_ms),
            });
        }
    }

    entries.sort_by(|a, b| b.last_active.cmp(&a.last_active));
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
                    "  No resumable sessions found (last 7 days)",
                    Style::default().fg(Color::DarkGray),
                )]))
                .block(block);
                f.render_widget(msg, chunks[0]);
            } else {
                let header = Row::new(vec![
                    Cell::from(" # "),
                    Cell::from("Session ID"),
                    Cell::from("Git(Project::Branch)"),
                    Cell::from("Model"),
                    Cell::from("Context"),
                    Cell::from("Last Active"),
                ])
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

                // Compute max git column width from actual data
                let git_col_width = entries.iter().map(|e| {
                    let project = dir_name(&e.cwd);
                    match &e.branch {
                        Some(b) => project.len() + 2 + b.len(), // "project::branch"
                        None => project.len(),
                    }
                }).max().unwrap_or(10) as u16 + 2; // +2 for padding

                let rows: Vec<Row> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let short_id = &e.session_id[..8.min(e.session_id.len())];

                        let project = dir_name(&e.cwd);
                        let project_cell = match &e.branch {
                            Some(b) => Cell::from(Line::from(vec![
                                Span::raw(project),
                                Span::styled("::", Style::default().fg(Color::DarkGray)),
                                Span::styled(b, Style::default().fg(Color::Green)),
                            ])),
                            None => Cell::from(project),
                        };

                        let model_display = e
                            .model
                            .as_deref()
                            .map(|m| model::display_name(m).to_string())
                            .unwrap_or_else(|| "—".to_string());

                        let window = e.model.as_deref()
                            .map(model::context_window)
                            .unwrap_or(200_000);
                        let tokens = format!("{}k / {}", e.tokens / 1000, crate::session::format_window(window));
                        let exited = format_relative(&e.last_active);

                        let row = Row::new(vec![
                            Cell::from(format!(" {} ", i + 1)),
                            Cell::from(short_id.to_string()),
                            project_cell,
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
                    Constraint::Length(4),              // #
                    Constraint::Length(12),             // Session ID
                    Constraint::Length(git_col_width),  // Git(Project::Branch)
                    Constraint::Length(14),             // Model
                    Constraint::Length(14),             // Context
                    Constraint::Min(12),               // Last Active
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
                            let name = dir_name(&entry.cwd);
                            result = Some((entry.session_id.clone(), name));
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

// --- Helpers ---

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

fn file_mtime_ms(path: &std::path::Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_epoch_ms(ms: u64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

struct JsonlSummary {
    model: Option<String>,
    branch: Option<String>,
    tokens: u64,
}

/// Read model, branch, and total tokens from the last assistant entry in a JSONL file.
/// Seeks to the tail of large files to avoid reading the entire file into memory.
fn read_jsonl_summary(path: &std::path::Path) -> JsonlSummary {
    use std::io::{BufReader, Seek, SeekFrom};
    use crate::session::read_line_capped;

    let empty = JsonlSummary { model: None, branch: None, tokens: 0 };
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return empty,
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut reader = BufReader::new(file);

    // Only read the last 1MB — we only need the last ~50 lines.
    const TAIL_BYTES: u64 = 1024 * 1024;
    if size > TAIL_BYTES {
        if reader.seek(SeekFrom::Start(size - TAIL_BYTES)).is_err() {
            return empty;
        }
        // Discard partial first line after seek
        let mut discard = String::new();
        let _ = read_line_capped(&mut reader, &mut discard);
    }

    // Heuristic: scan the last N lines for model/branch/tokens.
    // In practice the last assistant entry is near the end of the file.
    const TAIL_LINES: usize = 50;
    let mut ring = std::collections::VecDeque::with_capacity(TAIL_LINES);
    let mut line = String::new();
    loop {
        line.clear();
        match read_line_capped(&mut reader, &mut line) {
            Ok(0) => break,
            Ok(_) => {
                if !line.trim().is_empty() {
                    if ring.len() == TAIL_LINES {
                        ring.pop_front();
                    }
                    ring.push_back(std::mem::take(&mut line));
                }
            }
            Err(_) => break,
        }
    }

    let mut model = None;
    let mut branch = None;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;

    for line in ring.iter().rev() {
        // Pick up gitBranch from any recent entry
        if branch.is_none() && line.contains("\"gitBranch\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                branch = v.get("gitBranch").and_then(|b| b.as_str()).map(|s| s.to_string());
            }
        }

        if line.contains("\"type\":\"assistant\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(msg) = v.get("message") {
                    if model.is_none() {
                        model = msg.get("model").and_then(|m| m.as_str()).map(|s| s.to_string());
                    }
                    if input_tokens == 0 {
                        if let Some(usage) = msg.get("usage") {
                            input_tokens = usage.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0)
                                .saturating_add(usage.get("cache_creation_input_tokens").and_then(|t| t.as_u64()).unwrap_or(0))
                                .saturating_add(usage.get("cache_read_input_tokens").and_then(|t| t.as_u64()).unwrap_or(0));
                            output_tokens = usage.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                        }
                    }
                }
            }
            if model.is_some() && input_tokens > 0 && branch.is_some() {
                break;
            }
        }
    }

    JsonlSummary {
        model,
        branch,
        tokens: input_tokens.saturating_add(output_tokens),
    }
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
