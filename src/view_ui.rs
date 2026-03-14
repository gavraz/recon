use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
};

use crate::app::App;
use crate::session::{Session, SessionStatus};

// Layout constants
const ROOMS_PER_PAGE: usize = 4;
const SPRITE_W: usize = 10; // pixel columns
const SPRITE_H: usize = 10; // pixel rows
const SPRITE_RENDER_H: u16 = (SPRITE_H as u16 + 1) / 2; // terminal lines for sprite (5)
const CHAR_WIDTH: u16 = (SPRITE_W as u16) + 4; // sprite + padding
const CHAR_LABEL_LINES: u16 = 4; // name + branch + status + context bar
const CHAR_HEIGHT: u16 = SPRITE_RENDER_H + CHAR_LABEL_LINES;

// ── Pixel sprite data ────────────────────────────────────────────────
// Each sprite is SPRITE_H rows x SPRITE_W cols. 0 = transparent.
// Positive values index into the per-state color palette.
// Only Working and Input have multiple frames (animated).

type Sprite = [[u8; SPRITE_W]; SPRITE_H];
type Palette = &'static [(u8, u8, u8)]; // index 0 unused (transparent)

// Egg palette: 1=cream shell, 2=shadow, 3=green spots
const PAL_EGG: &[(u8, u8, u8)] = &[
    (0, 0, 0),         // 0: unused
    (255, 250, 230),    // 1: cream shell
    (220, 200, 170),    // 2: shell shadow
    (180, 220, 180),    // 3: green spots
];

const SPRITE_EGG: [Sprite; 1] = [[
    [0,0,0,0,1,1,1,0,0,0],
    [0,0,0,1,1,1,1,1,0,0],
    [0,0,1,1,1,3,1,1,1,0],
    [0,0,1,1,1,1,1,1,1,0],
    [0,0,1,3,1,1,1,3,1,0],
    [0,0,1,1,1,1,1,1,1,0],
    [0,0,1,1,1,1,1,1,1,0],
    [0,0,0,1,2,1,2,1,0,0],
    [0,0,0,0,1,1,1,0,0,0],
    [0,0,0,0,0,0,0,0,0,0],
]];

// Working palette: 1=green body, 2=dark green, 3=eyes, 4=eye highlight,
//                  5=blush, 6=mouth, 7=feet, 8=sparkle
const PAL_WORKING: &[(u8, u8, u8)] = &[
    (0, 0, 0),
    (120, 220, 120),    // 1: green body
    (80, 180, 80),      // 2: darker green
    (40, 40, 40),       // 3: eyes
    (255, 255, 255),    // 4: eye highlight
    (255, 150, 150),    // 5: cheeks
    (200, 100, 80),     // 6: mouth
    (100, 200, 100),    // 7: feet
    (255, 220, 60),     // 8: sparkle
];

const SPRITE_WORKING: [Sprite; 3] = [
    // Frame 0: happy, sparkles top
    [
        [0,0,0,8,1,1,1,8,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,1,3,4,1,1,3,4,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,5,1,1,6,6,1,1,5,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,0,7,0,0,7,0,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
    // Frame 1: squinting
    [
        [0,0,0,1,1,1,1,0,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,1,1,3,1,1,3,1,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,5,1,6,1,1,6,1,5,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,7,0,0,0,0,7,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
    // Frame 2: arms out, sparkles
    [
        [0,0,8,1,1,1,1,8,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,1,4,3,1,1,4,3,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,5,1,1,6,6,1,1,5,0],
        [8,1,1,1,1,1,1,1,1,8],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,0,7,0,0,7,0,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
];

// Idle palette: 1=blue-grey body, 2=darker, 3=closed eyes, 4=highlight, 5=feet, 6=Zzz
const PAL_IDLE: &[(u8, u8, u8)] = &[
    (0, 0, 0),
    (140, 160, 200),    // 1: blue-grey body
    (110, 130, 170),    // 2: darker
    (60, 60, 80),       // 3: closed eyes
    (180, 190, 220),    // 4: highlight
    (120, 140, 180),    // 5: feet
    (200, 200, 255),    // 6: Zzz
];

const SPRITE_IDLE: [Sprite; 1] = [[
    [0,0,0,1,1,1,1,0,0,0],
    [0,0,1,1,1,1,1,1,0,6],
    [0,1,1,1,1,1,1,1,1,0],
    [0,1,3,3,1,1,3,3,1,6],
    [0,1,1,1,1,1,1,1,1,0],
    [0,1,1,1,1,1,1,1,1,0],
    [0,1,1,1,1,1,1,1,1,0],
    [0,0,1,1,1,1,1,1,0,0],
    [0,0,0,5,0,0,5,0,0,0],
    [0,0,0,0,0,0,0,0,0,0],
]];

// Input (angry) palette: 1=orange body, 2=darker, 3=pupils, 4=eye whites,
//                        5=angry red, 6=feet, 7=flush
const PAL_INPUT: &[(u8, u8, u8)] = &[
    (0, 0, 0),
    (255, 180, 60),     // 1: orange body
    (220, 150, 40),     // 2: darker
    (40, 40, 40),       // 3: pupils
    (255, 255, 255),    // 4: eye whites
    (255, 60, 60),      // 5: angry red (brows, mouth)
    (200, 140, 40),     // 6: feet
    (255, 100, 100),    // 7: flush/anger
];

const SPRITE_INPUT: [Sprite; 3] = [
    // Frame 0: angry brows down
    [
        [0,0,0,1,1,1,1,0,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,5,1,1,1,1,5,1,0],
        [0,1,1,4,3,3,4,1,1,0],
        [0,7,1,1,1,1,1,1,7,0],
        [0,1,1,5,5,5,5,1,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,0,6,0,0,6,0,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
    // Frame 1: brows shifted
    [
        [0,0,0,1,1,1,1,0,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,1,5,1,1,5,1,1,0],
        [0,1,1,4,3,3,4,1,1,0],
        [0,7,1,1,1,1,1,1,7,0],
        [0,1,1,1,5,5,1,1,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,6,0,0,0,0,6,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
    // Frame 2: wider stance
    [
        [0,0,0,1,1,1,1,0,0,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,1,5,1,1,1,1,5,1,0],
        [0,1,1,3,4,4,3,1,1,0],
        [0,1,7,1,1,1,1,7,1,0],
        [0,1,5,1,5,5,1,5,1,0],
        [0,1,1,1,1,1,1,1,1,0],
        [0,0,1,1,1,1,1,1,0,0],
        [0,0,0,6,0,0,6,0,0,0],
        [0,0,0,0,0,0,0,0,0,0],
    ],
];

// ── Sprite selection ─────────────────────────────────────────────────

fn sprite_data(status: &SessionStatus, frame: usize) -> (&'static Sprite, Palette) {
    match status {
        SessionStatus::New => (&SPRITE_EGG[0], PAL_EGG),
        SessionStatus::Working => (&SPRITE_WORKING[frame % 3], PAL_WORKING),
        SessionStatus::Idle => (&SPRITE_IDLE[0], PAL_IDLE),
        SessionStatus::Input => (&SPRITE_INPUT[frame % 3], PAL_INPUT),
    }
}

// ── Half-block renderer ──────────────────────────────────────────────
// Renders a pixel grid as Lines of Spans using ▀▄ with fg+bg colors.
// Each terminal line encodes 2 pixel rows.

fn render_sprite_lines(sprite: &Sprite, palette: Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let rows = SPRITE_H;
    let cols = SPRITE_W;

    for y in (0..rows).step_by(2) {
        let mut spans: Vec<Span<'static>> = Vec::new();

        for x in 0..cols {
            let top = sprite[y][x];
            let bot = if y + 1 < rows { sprite[y + 1][x] } else { 0 };

            if top == 0 && bot == 0 {
                spans.push(Span::raw(" "));
            } else if top == 0 {
                // Bottom pixel only: ▄ with fg = bottom color
                let (r, g, b) = palette[bot as usize];
                spans.push(Span::styled(
                    "\u{2584}",
                    Style::default().fg(Color::Rgb(r, g, b)),
                ));
            } else if bot == 0 {
                // Top pixel only: ▀ with fg = top color
                let (r, g, b) = palette[top as usize];
                spans.push(Span::styled(
                    "\u{2580}",
                    Style::default().fg(Color::Rgb(r, g, b)),
                ));
            } else {
                // Both pixels: ▀ with fg = top, bg = bottom
                let (tr, tg, tb) = palette[top as usize];
                let (br, bg, bb) = palette[bot as usize];
                spans.push(Span::styled(
                    "\u{2580}",
                    Style::default()
                        .fg(Color::Rgb(tr, tg, tb))
                        .bg(Color::Rgb(br, bg, bb)),
                ));
            }
        }

        lines.push(Line::from(spans));
    }

    lines
}

// ── Room grouping ────────────────────────────────────────────────────

struct Room {
    name: String,
    session_indices: Vec<usize>,
    has_input: bool,
}

fn group_into_rooms(sessions: &[Session]) -> Vec<Room> {
    let mut map: BTreeMap<String, Vec<usize>> = BTreeMap::new();

    for (i, s) in sessions.iter().enumerate() {
        let room_name = if s.cwd.is_empty() {
            "unknown".to_string()
        } else {
            shorten_home(&s.cwd)
        };
        map.entry(room_name).or_default().push(i);
    }

    let mut rooms: Vec<Room> = map
        .into_iter()
        .map(|(name, indices)| {
            let has_input = indices
                .iter()
                .any(|&i| sessions[i].status == SessionStatus::Input);
            Room {
                name,
                session_indices: indices,
                has_input,
            }
        })
        .collect();

    rooms.sort_by(|a, b| {
        b.has_input
            .cmp(&a.has_input)
            .then_with(|| a.name.cmp(&b.name))
    });

    rooms
}

// ── Animation ────────────────────────────────────────────────────────

fn animation_frame(status: &SessionStatus, tick: u64) -> usize {
    match status {
        SessionStatus::Working => ((tick / 2) % 3) as usize,
        SessionStatus::Input => (tick % 3) as usize,
        _ => 0,
    }
}

fn session_phase_offset(session_id: &str) -> u64 {
    session_id
        .bytes()
        .fold(0u64, |a, b| a.wrapping_add(b as u64))
        % 7
}

fn status_color(status: &SessionStatus) -> Color {
    match status {
        SessionStatus::New => Color::Blue,
        SessionStatus::Working => Color::Green,
        SessionStatus::Idle => Color::DarkGray,
        SessionStatus::Input => Color::Yellow,
    }
}

// ── Context bar ──────────────────────────────────────────────────────

fn context_bar(ratio: f64) -> (String, Color) {
    let bar_width = 6usize;
    let filled = (ratio * bar_width as f64).round().min(bar_width as f64) as usize;
    let empty = bar_width - filled;
    let pct = (ratio * 100.0) as u32;
    let bar = format!(
        "{}{} {}%",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty),
        pct
    );
    let color = if ratio > 0.75 {
        Color::Red
    } else if ratio > 0.40 {
        Color::Yellow
    } else {
        Color::Green
    };
    (bar, color)
}

// ── Public render entry point ────────────────────────────────────────

pub fn resolve_zoom(app: &mut App) {
    let rooms = group_into_rooms(&app.sessions);
    let total_pages = (rooms.len() + ROOMS_PER_PAGE - 1) / ROOMS_PER_PAGE;
    if total_pages > 0 {
        app.view_page = app.view_page.min(total_pages - 1);
    } else {
        app.view_page = 0;
    }

    if let Some(idx) = app.view_zoom_index.take() {
        let page_start = app.view_page * ROOMS_PER_PAGE;
        if let Some(room) = rooms.get(page_start + idx) {
            app.view_zoomed_room = Some(room.name.clone());
        }
    }

    // Clamp agent selection within zoomed room bounds
    if let Some(ref zoomed_name) = app.view_zoomed_room {
        if let Some(room) = rooms.iter().find(|r| &r.name == zoomed_name) {
            if !room.session_indices.is_empty() {
                app.view_selected_agent =
                    app.view_selected_agent.min(room.session_indices.len() - 1);
            } else {
                app.view_selected_agent = 0;
            }
        }
    }
}

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    render_rooms(frame, app, chunks[0]);
    render_footer(frame, app, chunks[1]);
}

fn render_rooms(frame: &mut Frame, app: &App, area: Rect) {
    let rooms = group_into_rooms(&app.sessions);

    if rooms.is_empty() {
        render_empty(frame, area, app.tick);
        return;
    }

    if let Some(ref zoomed_name) = app.view_zoomed_room {
        if let Some(room) = rooms.iter().find(|r| &r.name == zoomed_name) {
            render_room(frame, app, room, area, None, Some(app.view_selected_agent));
            return;
        }
    }

    let total_pages = (rooms.len() + ROOMS_PER_PAGE - 1) / ROOMS_PER_PAGE;
    let page = app.view_page.min(total_pages.saturating_sub(1));
    let page_start = page * ROOMS_PER_PAGE;
    let page_rooms: Vec<&Room> = rooms
        .iter()
        .skip(page_start)
        .take(ROOMS_PER_PAGE)
        .collect();

    let v_chunks = Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(area);
    let top_h = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(v_chunks[0]);
    let bot_h = Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(v_chunks[1]);
    let cells = [top_h[0], top_h[1], bot_h[0], bot_h[1]];

    for (i, cell) in cells.iter().enumerate() {
        if let Some(room) = page_rooms.get(i) {
            render_room(frame, app, room, *cell, Some(i + 1), None);
        } else {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(30, 30, 30)));
            frame.render_widget(block, *cell);
        }
    }
}

fn render_room(frame: &mut Frame, app: &App, room: &Room, area: Rect, slot_num: Option<usize>, selected_agent: Option<usize>) {
    let border_color = if room.has_input {
        if app.tick % 2 == 0 { Color::Yellow } else { Color::White }
    } else {
        Color::DarkGray
    };

    let title = match slot_num {
        Some(n) => format!(" [{}] {} ({}) ", n, room.name, room.session_indices.len()),
        None => format!(" {} ({}) ", room.name, room.session_indices.len()),
    };
    let title_style = if room.has_input {
        Style::default().fg(border_color)
    } else {
        Style::default().fg(Color::White)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title, title_style))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let chars_per_row = (inner.width / CHAR_WIDTH).max(1) as usize;
    let char_rows: Vec<&[usize]> = room.session_indices.chunks(chars_per_row).collect();

    let needed_height = char_rows.len() as u16 * CHAR_HEIGHT;
    let v_pad = inner.height.saturating_sub(needed_height) / 2;
    let char_area = Rect {
        x: inner.x,
        y: inner.y + v_pad,
        width: inner.width,
        height: inner.height.saturating_sub(v_pad),
    };

    let row_constraints: Vec<Constraint> = char_rows
        .iter()
        .map(|_| Constraint::Length(CHAR_HEIGHT))
        .collect();
    let v_chunks = Layout::vertical(row_constraints).split(char_area);

    for (row_idx, indices) in char_rows.iter().enumerate() {
        if row_idx >= v_chunks.len() {
            break;
        }
        let col_constraints: Vec<Constraint> = indices
            .iter()
            .map(|_| Constraint::Length(CHAR_WIDTH))
            .collect();
        let h_chunks = Layout::horizontal(col_constraints).split(v_chunks[row_idx]);

        for (col_idx, &session_idx) in indices.iter().enumerate() {
            if col_idx >= h_chunks.len() {
                break;
            }
            let flat_idx = row_idx * chars_per_row + col_idx;
            let is_selected = selected_agent == Some(flat_idx);
            render_character(frame, &app.sessions[session_idx], h_chunks[col_idx], app.tick, is_selected);
        }
    }
}

fn render_character(frame: &mut Frame, session: &Session, area: Rect, tick: u64, is_selected: bool) {
    if area.height < 3 || area.width < 4 {
        return;
    }

    let offset = session_phase_offset(&session.session_id);
    let anim_frame = animation_frame(&session.status, tick + offset);
    let (sprite, palette) = sprite_data(&session.status, anim_frame);
    let ratio = session.token_ratio();

    let color = if session.status == SessionStatus::Input {
        if tick % 2 == 0 { Color::Yellow } else { Color::White }
    } else {
        status_color(&session.status)
    };

    // Selection highlight background
    if is_selected {
        let bg = Block::default()
            .style(Style::default().bg(Color::Rgb(40, 40, 60)));
        frame.render_widget(bg, area);
    }

    let mut lines: Vec<Line> = Vec::new();

    // Pixel art sprite (5 terminal lines)
    let sprite_lines = render_sprite_lines(sprite, palette);
    lines.extend(sprite_lines);

    // Session name
    let name = session.tmux_session.as_deref().unwrap_or("???");
    let name_style = if is_selected {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::from(Span::styled(
        truncate_str(name, area.width as usize),
        name_style,
    )));

    // Git branch
    let branch = session.branch.as_deref().unwrap_or("");
    lines.push(Line::from(Span::styled(
        truncate_str(branch, area.width as usize),
        Style::default().fg(Color::Green),
    )));

    // Status label
    lines.push(Line::from(Span::styled(
        session.status.label(),
        Style::default().fg(color),
    )));

    // Context bar
    let (bar_str, bar_color) = context_bar(ratio);
    lines.push(Line::from(Span::styled(
        truncate_str(&bar_str, area.width as usize),
        Style::default().fg(bar_color),
    )));

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn render_empty(frame: &mut Frame, area: Rect, _tick: u64) {
    let (sprite, palette) = sprite_data(&SessionStatus::Idle, 0);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.extend(render_sprite_lines(sprite, palette));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "No active sessions",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let rooms = group_into_rooms(&app.sessions);
    let total_pages = (rooms.len() + ROOMS_PER_PAGE - 1) / ROOMS_PER_PAGE;
    let page = app.view_page.min(total_pages.saturating_sub(1));

    let mut spans = vec![];

    if app.view_zoomed_room.is_some() {
        spans.push(Span::styled("h/l", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" select  "));
        spans.push(Span::styled("Enter", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" switch  "));
        spans.push(Span::styled("x", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" kill  "));
        spans.push(Span::styled("n", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" new  "));
        spans.push(Span::styled("Esc", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" back  "));
    } else {
        spans.push(Span::styled("1-4", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" zoom  "));
        if total_pages > 1 {
            spans.push(Span::styled("j/k", Style::default().fg(Color::Cyan)));
            spans.push(Span::raw(format!(" page ({}/{})  ", page + 1, total_pages)));
        }
    }

    spans.push(Span::styled("v", Style::default().fg(Color::Cyan)));
    spans.push(Span::raw(" table  "));
    spans.push(Span::styled("r", Style::default().fg(Color::Cyan)));
    spans.push(Span::raw(" refresh  "));
    spans.push(Span::styled("q", Style::default().fg(Color::Cyan)));
    spans.push(Span::raw(" quit"));

    let footer = Paragraph::new(Line::from(spans));
    frame.render_widget(footer, area);
}

// ── Helpers ──────────────────────────────────────────────────────────

fn shorten_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

fn truncate_str(s: &str, max_width: usize) -> String {
    let char_count: usize = s.chars().count();
    if char_count <= max_width {
        s.to_string()
    } else if max_width > 1 {
        let truncated: String = s.chars().take(max_width - 1).collect();
        format!("{}\u{2026}", truncated)
    } else {
        String::new()
    }
}
