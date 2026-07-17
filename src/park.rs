use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::tmux;

#[derive(Serialize, Deserialize)]
struct ParkFile {
    parked_at: String,
    sessions: Vec<ParkedSession>,
}

#[derive(Serialize, Deserialize)]
struct ParkedSession {
    session_id: String,
    tmux_session: String,
    cwd: String,
}

fn park_file_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| {
        h.join(".local")
            .join("state")
            .join("recon")
            .join("parked.json")
    })
}

pub fn park() {
    let mut app = App::new();
    app.refresh();

    let parked: Vec<ParkedSession> = app
        .sessions
        .iter()
        .filter_map(|s| {
            // Use the session ID from the JSONL filename, not s.session_id.
            // For resumed sessions, s.session_id is a new ID but the JSONL
            // (and `claude --resume`) uses the original session ID.
            let resume_id = s
                .jsonl_path
                .file_stem()
                .and_then(|f| f.to_str())
                .map(|f| f.to_string())
                .unwrap_or_else(|| s.session_id.clone());
            Some(ParkedSession {
                session_id: resume_id,
                tmux_session: s.tmux_session.as_ref()?.clone(),
                cwd: s.cwd.clone(),
            })
        })
        .collect();

    if parked.is_empty() {
        eprintln!("No live sessions to park.");
        return;
    }

    let park_file = ParkFile {
        parked_at: chrono::Utc::now().to_rfc3339(),
        sessions: parked,
    };

    let path = match park_file_path() {
        Some(p) => p,
        None => {
            eprintln!("Could not determine home directory.");
            return;
        }
    };

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Failed to create directory: {e}");
            return;
        }
    }

    let json = match serde_json::to_string_pretty(&park_file) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Failed to serialize: {e}");
            return;
        }
    };

    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("Failed to write park file: {e}");
        return;
    }

    eprintln!(
        "Parked {} session(s) to {}",
        park_file.sessions.len(),
        path.display()
    );
    for s in &park_file.sessions {
        eprintln!(
            "  {} ({})",
            s.tmux_session,
            &s.session_id[..8.min(s.session_id.len())]
        );
    }
}

pub fn unpark() {
    let path = match park_file_path() {
        Some(p) => p,
        None => {
            eprintln!("Could not determine home directory.");
            return;
        }
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("Nothing parked.");
            return;
        }
    };

    let park_file: ParkFile = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to read park file: {e}");
            return;
        }
    };

    if park_file.sessions.is_empty() {
        eprintln!("Park file is empty.");
        let _ = std::fs::remove_file(&path);
        return;
    }

    eprintln!(
        "Restoring {} session(s) from {}...",
        park_file.sessions.len(),
        park_file.parked_at
    );

    for s in &park_file.sessions {
        match tmux::resume_session(&s.session_id, Some(&s.tmux_session)) {
            Ok(name) => {
                eprintln!(
                    "  Restored {} ({})",
                    name,
                    &s.session_id[..8.min(s.session_id.len())]
                );
            }
            Err(e) => {
                eprintln!("  Failed to restore {}: {e}", s.tmux_session);
            }
        }
    }

    eprintln!("Done. Park file kept at {}", path.display());
}
