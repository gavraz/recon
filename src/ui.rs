use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::app::App;
use crate::config::Column;
use crate::session::{Session, SessionStatus};

pub fn render(frame: &mut Frame, app: &App) {
    let show_search = app.filter_active || !app.filter_text.is_empty();
    let chunks = if show_search {
        Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area())
    } else {
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area())
    };

    render_table(frame, app, chunks[0]);
    if show_search {
        render_search_bar(frame, app, chunks[1]);
        render_footer(frame, app, chunks[2]);
    } else {
        render_footer(frame, app, chunks[1]);
    }
}

fn render_table(frame: &mut Frame, app: &App, area: Rect) {
    let columns = &app.config.table.columns;

    // The `#` index column is always shown first and is not configurable.
    let mut header_cells = vec![Cell::from(" # ")];
    header_cells.extend(columns.iter().map(|c| Cell::from(c.header())));
    let header = Row::new(header_cells).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let filtered = app.filtered_indices();
    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(display_idx, &real_idx)| {
            let session = &app.sessions[real_idx];
            let num = format!(" {} ", real_idx + 1);

            let mut cells = vec![Cell::from(num)];
            cells.extend(columns.iter().map(|&col| render_column(col, session)));
            let row = Row::new(cells);

            if session.status == SessionStatus::Input {
                row.style(Style::default().bg(Color::Rgb(50, 40, 0)))
            } else if display_idx == app.selected {
                // Muted blue-gray, not DarkGray: the Directory text and the
                // Project "::" separators are drawn in DarkGray, so a DarkGray
                // highlight would render them invisible on the selected row.
                row.style(Style::default().bg(Color::Rgb(45, 50, 70)))
            } else {
                row
            }
        })
        .collect();

    let mut widths = vec![Constraint::Length(4)]; // #
    widths.extend(columns.iter().map(|c| column_constraint(*c, app)));

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" recon — Claude Code Sessions "),
    );

    frame.render_widget(table, area);
}

/// Width for a column: a user override (if any) or the built-in default.
fn column_constraint(col: Column, app: &App) -> Constraint {
    if let Some(&w) = app.config.table.widths.get(&col) {
        return Constraint::Length(w);
    }
    match col {
        Column::Session => Constraint::Length(16),
        Column::Window => Constraint::Length(16),
        Column::Project => Constraint::Min(20),
        Column::Directory => Constraint::Length(20),
        Column::Status => Constraint::Length(10),
        Column::Model => Constraint::Length(20),
        Column::Context => Constraint::Length(14),
        Column::LastActivity => Constraint::Length(14),
    }
}

/// Render a single table cell for the given column of a session.
fn render_column(col: Column, session: &Session) -> Cell<'static> {
    match col {
        Column::Session => {
            let name = session.tmux_session.as_deref().unwrap_or("—");
            Cell::from(name.to_string())
        }
        Column::Window => {
            let name = session
                .tmux_window
                .as_deref()
                .filter(|w| !w.is_empty())
                .unwrap_or("—");
            Cell::from(name.to_string()).style(Style::default().fg(Color::Magenta))
        }
        Column::Project => {
            // repo::relative_dir::branch
            let mut spans = vec![Span::raw(session.project_name.clone())];
            if let Some(dir) = &session.relative_dir {
                spans.push(Span::styled("::", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(dir.clone(), Style::default().fg(Color::Cyan)));
            }
            if let Some(b) = &session.branch {
                spans.push(Span::styled("::", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(b.clone(), Style::default().fg(Color::Green)));
            }
            Cell::from(Line::from(spans))
        }
        Column::Directory => {
            Cell::from(shorten_home(&session.cwd)).style(Style::default().fg(Color::DarkGray))
        }
        Column::Status => {
            let (dot, label, color) = match session.status {
                SessionStatus::New => ("●", "New", Color::Blue),
                SessionStatus::Working => ("●", "Working", Color::Green),
                SessionStatus::Idle => ("●", "Idle", Color::DarkGray),
                SessionStatus::Input => ("●", "Input", Color::Yellow),
            };
            Cell::from(Line::from(vec![
                Span::styled(dot, Style::default().fg(color)),
                Span::styled(format!(" {label}"), Style::default().fg(color)),
            ]))
        }
        Column::Model => Cell::from(session.model_display()),
        Column::Context => {
            let token_ratio = session.token_ratio();
            let style = if token_ratio > 0.9 {
                Style::default().fg(Color::Red)
            } else if token_ratio > 0.75 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Cell::from(session.token_display()).style(style)
        }
        Column::LastActivity => {
            let activity = session
                .last_activity
                .as_deref()
                .map(format_timestamp)
                .unwrap_or_else(|| "—".to_string());
            Cell::from(activity)
        }
    }
}

fn render_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::raw(&app.filter_text),
    ];
    if !app.filter_active && !app.filter_text.is_empty() {
        let count = app.filtered_indices().len();
        spans.push(Span::styled(
            format!("  ({} match{})", count, if count == 1 { "" } else { "es" }),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);

    if app.filter_active {
        frame.set_cursor_position((area.x + 1 + app.filter_cursor as u16, area.y));
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let spans = if app.filter_active {
        vec![
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::raw(" clear  "),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::raw(" keep filter  "),
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate"),
        ]
    } else {
        vec![
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  "),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::raw(" switch  "),
            Span::styled("x", Style::default().fg(Color::Cyan)),
            Span::raw(" kill  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("v", Style::default().fg(Color::Cyan)),
            Span::raw(" view  "),
            Span::styled("i", Style::default().fg(Color::Cyan)),
            Span::raw(" next input  "),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ]
    };
    let footer = Paragraph::new(Line::from(spans));
    frame.render_widget(footer, area);
}

/// Replace home directory prefix with ~.
fn shorten_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Format an ISO timestamp into a relative or short time string.
fn format_timestamp(ts: &str) -> String {
    use chrono::{DateTime, Local, Utc};

    let parsed = ts.parse::<DateTime<Utc>>();
    match parsed {
        Ok(dt) => {
            let now = Utc::now();
            let diff = now - dt;

            if diff.num_seconds() < 60 {
                "< 1m".to_string()
            } else if diff.num_minutes() < 60 {
                format!("{}m ago", diff.num_minutes())
            } else if diff.num_hours() < 24 {
                format!("{}h ago", diff.num_hours())
            } else {
                dt.with_timezone(&Local).format("%b %d %H:%M").to_string()
            }
        }
        Err(_) => ts.to_string(),
    }
}
