mod app;
mod cli;
mod history;
mod model;
mod new_session;
mod park;
mod session;
mod tmux;
mod ui;
mod view_ui;

use std::io;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;

use app::{App, ViewMode};
use cli::{Cli, Command};

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::New) => {
            let result = new_session::run_new_session_form()?;
            if let Some(name) = result {
                tmux::switch_to_session(&name);
            }
        }
        Some(Command::Launch { name_only }) => {
            let (default_name, cwd) = tmux::default_new_session_info();
            match tmux::create_session(&default_name, &cwd) {
                Ok(name) => {
                    if name_only {
                        print!("{name}");
                    } else {
                        tmux::switch_to_session(&name);
                        eprintln!("Session: {name}");
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some(Command::Resume { id, name, no_attach }) => {
            if let Some(session_id) = id {
                match tmux::resume_session(&session_id, name.as_deref()) {
                    Ok(sess) => {
                        if !no_attach {
                            tmux::switch_to_session(&sess);
                        }
                        eprintln!("Resumed in session: {sess}");
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                let result = history::run_resume_picker()?;
                if let Some((session_id, sess_name)) = result {
                    match tmux::resume_session(&session_id, Some(&sess_name)) {
                        Ok(sess) => {
                            tmux::switch_to_session(&sess);
                            eprintln!("Resumed in session: {sess}");
                        }
                        Err(e) => {
                            eprintln!("Error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Some(Command::Next) => {
            let mut app = App::new();
            app.refresh();
            if let Some(session) = app.sessions.iter().find(|s| s.status == session::SessionStatus::Input) {
                if let Some(target) = &session.pane_target {
                    tmux::switch_to_session(target);
                }
            }
        }
        Some(Command::Json) => {
            let mut app = App::new();
            app.refresh();
            println!("{}", app.to_json());
        }
        Some(Command::Park) => {
            park::park();
        }
        Some(Command::Unpark) => {
            park::unpark();
        }
        Some(Command::View) | None => {
            let start_mode = if matches!(cli.command, Some(Command::View)) {
                ViewMode::View
            } else {
                ViewMode::Table
            };
            run_tui(start_mode)?;
        }
    }

    Ok(())
}

fn run_tui(start_mode: ViewMode) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, start_mode);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }

    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, start_mode: ViewMode) -> io::Result<()> {
    let mut app = App::new();
    app.view_mode = start_mode;
    app.refresh();

    let refresh_interval = Duration::from_secs(2);
    let mut last_refresh = Instant::now();

    loop {
        if app.view_mode == ViewMode::View {
            view_ui::resolve_zoom(&mut app);
        }
        terminal.draw(|f| {
            match app.view_mode {
                ViewMode::Table => ui::render(f, &app),
                ViewMode::View => view_ui::render(f, &app),
            }
        })?;

        app.advance_tick();

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }

        if app.should_quit {
            return Ok(());
        }

        if last_refresh.elapsed() >= refresh_interval {
            app.refresh();
            last_refresh = Instant::now();
        }
    }
}
