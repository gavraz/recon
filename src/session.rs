use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::model;
use crate::tmux;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    New,
    Working,
    Idle,
    Input,
}

impl SessionStatus {
    pub fn label(&self) -> &str {
        match self {
            SessionStatus::New => "New",
            SessionStatus::Working => "Working",
            SessionStatus::Idle => "Idle",
            SessionStatus::Input => "Input",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub project_name: String,
    pub cwd: String,
    pub tmux_session: Option<String>,
    pub model: Option<String>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub status: SessionStatus,
    pub pid: Option<i32>,
    pub last_activity: Option<String>,
    pub jsonl_path: PathBuf,
    pub last_file_size: u64,
}

impl Session {
    pub fn token_display(&self) -> String {
        let used = self.total_input_tokens + self.total_output_tokens;
        let window = self
            .model
            .as_deref()
            .map(model::context_window)
            .unwrap_or(200_000);
        format!("{}k / {}k", used / 1000, window / 1000)
    }

    pub fn token_ratio(&self) -> f64 {
        let used = self.total_input_tokens + self.total_output_tokens;
        let window = self
            .model
            .as_deref()
            .map(model::context_window)
            .unwrap_or(200_000);
        if window == 0 {
            return 0.0;
        }
        used as f64 / window as f64
    }

    pub fn model_display(&self, effort: &str) -> String {
        match &self.model {
            Some(m) => model::format_with_effort(m, effort),
            None => "—".to_string(),
        }
    }
}

/// Discover sessions by scanning JSONL files, then matching to live processes and tmux.
pub fn discover_sessions(prev_sessions: &HashMap<String, Session>) -> Vec<Session> {
    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return vec![],
    };

    if !claude_dir.exists() {
        return vec![];
    }

    // Get live claude processes and tmux mapping
    let live_procs = discover_live_claude_procs();
    let tty_map = tmux::tty_to_session_map();

    let mut sessions = Vec::new();
    let mut claimed_pids: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);

    // Collect all candidate JSONL files (skip subdirectories like subagents/)
    let mut candidates: Vec<(PathBuf, PathBuf, String, SystemTime)> = Vec::new();

    let entries = match fs::read_dir(&claude_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        // Only scan direct JSONL files, skip subdirectories (subagents, etc.)
        let jsonl_files = match fs::read_dir(&project_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for jentry in jsonl_files.flatten() {
            let path = jentry.path();
            if path.is_dir() {
                continue;
            }
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                let modified = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);

                if modified < cutoff {
                    continue;
                }

                let session_id = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                candidates.push((path, project_dir.clone(), session_id, modified));
            }
        }
    }

    // Sort by modification time (newest first) so the most recent JSONL
    // for a given CWD claims the process first.
    candidates.sort_by(|a, b| b.3.cmp(&a.3));

    for (path, project_dir, session_id, _modified) in candidates {
        // Check if we have a previous session for incremental parsing
        let prev = prev_sessions.get(&session_id);
        let prev_file_size = prev.map(|s| s.last_file_size).unwrap_or(0);
        let prev_input = prev.map(|s| s.total_input_tokens).unwrap_or(0);
        let prev_output = prev.map(|s| s.total_output_tokens).unwrap_or(0);
        let prev_model = prev.and_then(|s| s.model.clone());

        // Parse the JSONL
        let info = parse_jsonl(
            &path,
            prev_file_size,
            prev_input,
            prev_output,
            prev_model,
        );

        let cwd = info
            .cwd
            .unwrap_or_else(|| decode_project_path(&project_dir));
        let project_name = shorten_path(&cwd);

        // Match to a live process (skip already-claimed PIDs)
        let proc = find_matching_process(&live_procs, &session_id, &cwd, &claimed_pids);

        // Only show sessions with a live process
        let pid = match proc {
            Some(p) => p.pid,
            None => continue,
        };

        claimed_pids.insert(pid);

        // Map to tmux session via TTY
        let tmux_session = proc
            .and_then(|p| tty_map.get(&p.tty).cloned());

        let status = determine_status(
            &path,
            info.input_tokens,
            info.output_tokens,
            tmux_session.as_deref(),
        );

        sessions.push(Session {
            session_id,
            project_name,
            cwd,
            tmux_session,
            model: info.model,
            total_input_tokens: info.input_tokens,
            total_output_tokens: info.output_tokens,
            status,
            pid: Some(pid),
            last_activity: info.last_activity,
            jsonl_path: path,
            last_file_size: info.file_size,
        });
    }

    // Also discover tmux sessions running claude that have no JSONL yet
    let known_tmux: std::collections::HashSet<String> = sessions
        .iter()
        .filter_map(|s| s.tmux_session.clone())
        .collect();

    for (tmux_name, pane_cwd) in discover_claude_tmux_sessions() {
        if known_tmux.contains(&tmux_name) {
            continue;
        }
        let project_name = std::path::Path::new(&pane_cwd)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| tmux_name.clone());

        sessions.push(Session {
            session_id: format!("tmux-{tmux_name}"),
            project_name,
            cwd: pane_cwd,
            tmux_session: Some(tmux_name),
            model: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            status: SessionStatus::New,
            pid: None,
            last_activity: None,
            jsonl_path: PathBuf::new(),
            last_file_size: 0,
        });
    }

    // Sort by project name for stable ordering
    sessions.sort_by(|a, b| a.project_name.cmp(&b.project_name));
    sessions
}

#[derive(Debug)]
struct ParsedInfo {
    input_tokens: u64,
    output_tokens: u64,
    model: Option<String>,
    cwd: Option<String>,
    last_activity: Option<String>,
    status: SessionStatus,
    file_size: u64,
}

/// Shorten a path by replacing the home directory with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Decode an encoded project directory name back to a path.
/// `-Users-gavra-repos-yaba` -> `/Users/gavra/repos/yaba`
/// This is a best-effort reverse of the encoding (ambiguous for `.` and `_`).
fn decode_project_path(project_dir: &Path) -> String {
    let name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // The encoded name replaces / with -, so the first char is always -
    // Convert back: leading - becomes /, internal - becomes /
    // This is lossy (can't distinguish original - from / or . or _) but good enough
    if name.starts_with('-') {
        name.replacen('-', "/", 1)
            .replace('-', "/")
    } else {
        name
    }
}

/// Minimal serde structs for JSONL parsing.
#[derive(Deserialize)]
struct JsonlEntry {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    message: Option<MessageEntry>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct MessageEntry {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<UsageEntry>,
}

#[derive(Deserialize)]
struct UsageEntry {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Parse JSONL file, incrementally if possible.
fn parse_jsonl(
    path: &Path,
    prev_file_size: u64,
    prev_input: u64,
    prev_output: u64,
    prev_model: Option<String>,
) -> ParsedInfo {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            return ParsedInfo {
                input_tokens: prev_input,
                output_tokens: prev_output,
                model: prev_model,
                cwd: None,
                last_activity: None,
                status: SessionStatus::Idle,
                file_size: 0,
            }
        }
    };

    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    if file_size == prev_file_size && prev_file_size > 0 {
        return ParsedInfo {
            input_tokens: prev_input,
            output_tokens: prev_output,
            model: prev_model,
            cwd: None,
            last_activity: None,
            status: SessionStatus::Idle, // placeholder, resolved later
            file_size,
        };
    }

    let mut reader = BufReader::new(file);
    let mut total_input = prev_input;
    let mut total_output = prev_output;
    let mut model = prev_model;
    let mut last_activity = None;
    let mut cwd = None;

    if prev_file_size > 0 {
        let _ = reader.seek(SeekFrom::Start(prev_file_size));
    } else {
        total_input = 0;
        total_output = 0;
        model = None;
    }

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.contains("\"type\"") {
            continue;
        }

        if trimmed.contains("\"type\":\"assistant\"") {
            if let Ok(entry) = serde_json::from_str::<JsonlEntry>(trimmed) {
                if let Some(ts) = entry.timestamp {
                    last_activity = Some(ts);
                }
                if entry.cwd.is_some() {
                    cwd = entry.cwd;
                }
                if let Some(msg) = entry.message {
                    if let Some(m) = msg.model {
                        model = Some(m);
                    }
                    if let Some(usage) = msg.usage {
                        total_input = usage.input_tokens
                            + usage.cache_creation_input_tokens
                            + usage.cache_read_input_tokens;
                        total_output = usage.output_tokens;
                    }
                }
            }
        } else if trimmed.contains("\"type\":\"user\"") || trimmed.contains("\"type\":\"system\"") {
            if let Ok(entry) = serde_json::from_str::<JsonlEntry>(trimmed) {
                if let Some(ts) = entry.timestamp {
                    last_activity = Some(ts);
                }
                if entry.cwd.is_some() {
                    cwd = entry.cwd;
                }
            }
        }
    }

    // Get status from the last entry in the file
    let status = SessionStatus::Idle; // placeholder, resolved later

    ParsedInfo {
        input_tokens: total_input,
        output_tokens: total_output,
        model,
        cwd,
        last_activity,
        status,
        file_size,
    }
}

/// Determine session status from file recency and token counts.
/// - New: no tokens yet (never interacted)
/// - Working: JSONL modified in last 5s
/// - Input: last activity within 10 minutes (active conversation, waiting for user)
/// - Idle: last activity older than 10 minutes
fn determine_status(_path: &Path, input_tokens: u64, output_tokens: u64, tmux_session: Option<&str>) -> SessionStatus {
    if input_tokens == 0 && output_tokens == 0 {
        return SessionStatus::New;
    }

    // tmux pane content is the source of truth
    if let Some(name) = tmux_session {
        pane_status(name)
    } else {
        SessionStatus::Idle
    }
}

/// Determine status by inspecting the tmux pane content.
fn pane_status(session_name: &str) -> SessionStatus {
    let output = match std::process::Command::new("tmux")
        .args(["capture-pane", "-t", session_name, "-p", "-S", "-5"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return SessionStatus::Idle,
    };

    let content = String::from_utf8_lossy(&output.stdout);

    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Agent is running a tool/command
        if trimmed.contains("(running)") || trimmed.contains("Streaming") {
            return SessionStatus::Working;
        }
        // Permission prompt — blocked on user action
        if trimmed.starts_with("Do you want to")
            || trimmed.starts_with("❯ 1.")
            || trimmed.contains("Esc to cancel")
        {
            return SessionStatus::Input;
        }
        break;
    }

    SessionStatus::Idle
}

// --- Live process discovery (minimal, just for liveness check) ---

#[derive(Debug)]
struct LiveProcess {
    pid: i32,
    tty: String,
    session_id: Option<String>,
    cwd: Option<String>,
}

fn discover_live_claude_procs() -> Vec<LiveProcess> {
    let output = match std::process::Command::new("ps")
        .args(["-eo", "pid,tty,args"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut procs = Vec::new();

    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let args_joined = parts[2..].join(" ");
        if !is_claude_binary(&args_joined) {
            continue;
        }

        let pid: i32 = match parts[0].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let tty = parts[1].to_string();
        let session_id = extract_session_id(&args_joined);
        let cwd = get_process_cwd(pid);

        procs.push(LiveProcess {
            pid,
            tty,
            session_id,
            cwd,
        });
    }

    procs
}

fn find_matching_process<'a>(
    procs: &'a [LiveProcess],
    session_id: &str,
    cwd: &str,
    claimed_pids: &std::collections::HashSet<i32>,
) -> Option<&'a LiveProcess> {
    // First try matching by session_id in args
    if let Some(p) = procs.iter().find(|p| {
        !claimed_pids.contains(&p.pid)
            && p.session_id
                .as_ref()
                .map(|id| id == session_id)
                .unwrap_or(false)
    }) {
        return Some(p);
    }

    // Fall back to matching by CWD
    procs.iter().find(|p| {
        !claimed_pids.contains(&p.pid)
            && p.cwd
                .as_ref()
                .map(|c| c == cwd)
                .unwrap_or(false)
    })
}

fn is_claude_binary(args: &str) -> bool {
    if args.contains("node_modules/.bin/claude") {
        return true;
    }
    let first_arg = args.split_whitespace().next().unwrap_or("");
    if first_arg.ends_with("/claude") || first_arg == "claude" {
        return !first_arg.contains("claude-");
    }
    false
}

fn extract_session_id(args: &str) -> Option<String> {
    let parts: Vec<&str> = args.split_whitespace().collect();
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i] == "--resume" || parts[i] == "-r" {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

use std::sync::Mutex;

static CWD_CACHE: Mutex<Option<HashMap<i32, String>>> = Mutex::new(None);

/// Find tmux sessions whose pane is running claude (by pane_current_command).
/// Returns Vec<(session_name, pane_cwd)>.
fn discover_claude_tmux_sessions() -> Vec<(String, String)> {
    let output = match std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_current_command}\t#{pane_current_path}",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let session_name = parts[0];
        let command = parts[1];
        let pane_path = parts[2];

        // Claude shows up as a version number (e.g. "2.1.76") or "claude" or "node"
        let is_claude = command.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
            || command == "claude"
            || command == "node";

        if is_claude {
            results.push((session_name.to_string(), pane_path.to_string()));
        }
    }

    results
}

fn get_process_cwd(pid: i32) -> Option<String> {
    {
        let cache = CWD_CACHE.lock().unwrap();
        if let Some(cwd) = cache.as_ref().and_then(|c| c.get(&pid)) {
            return Some(cwd.clone());
        }
    }

    let output = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix('n') {
            if path.starts_with('/') {
                let mut cache = CWD_CACHE.lock().unwrap();
                if cache.is_none() {
                    *cache = Some(HashMap::new());
                }
                cache.as_mut().unwrap().insert(pid, path.to_string());
                return Some(path.to_string());
            }
        }
    }
    None
}
