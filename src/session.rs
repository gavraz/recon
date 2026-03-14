use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::model;

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
    pub branch: Option<String>,
    pub cwd: String,
    pub tmux_session: Option<String>,
    pub model: Option<String>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub status: SessionStatus,
    pub pid: Option<i32>,
    pub last_activity: Option<String>,
    pub started_at: u64,
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

/// Discover sessions by scanning JSONL files, then matching to live tmux panes.
pub fn discover_sessions(prev_sessions: &HashMap<String, Session>) -> Vec<Session> {
    let claude_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return vec![],
    };

    if !claude_dir.exists() {
        return vec![];
    }

    // Build the live session map: session_id → (pid, tmux_name, started_at)
    // by joining ~/.claude/sessions/{PID}.json with tmux pane info.
    let live_map = build_live_session_map();

    let mut sessions = Vec::new();
    let mut matched_session_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);

    // Scan all JSONL files across project directories
    let entries = match fs::read_dir(&claude_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        let jsonl_files = match fs::read_dir(&project_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for jentry in jsonl_files.flatten() {
            let path = jentry.path();
            if path.is_dir() {
                continue;
            }
            if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                continue;
            }

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

            // Look up in live map — skip if no live process
            let live = match live_map.get(&session_id) {
                Some(l) => l,
                None => continue,
            };

            // Incremental JSONL parsing
            let prev = prev_sessions.get(&session_id);
            let info = parse_jsonl(
                &path,
                prev.map(|s| s.last_file_size).unwrap_or(0),
                prev.map(|s| s.total_input_tokens).unwrap_or(0),
                prev.map(|s| s.total_output_tokens).unwrap_or(0),
                prev.and_then(|s| s.model.clone()),
                prev.and_then(|s| s.last_activity.clone()),
            );

            let cwd = info
                .cwd
                .unwrap_or_else(|| decode_project_path(&project_dir));
            let (project_name, branch) = git_project_info(&cwd);

            let status = determine_status(
                &path,
                info.input_tokens,
                info.output_tokens,
                Some(&live.tmux_session),
            );

            matched_session_ids.insert(session_id.clone());

            sessions.push(Session {
                session_id,
                project_name,
                branch,
                cwd,
                tmux_session: Some(live.tmux_session.clone()),
                model: info.model,
                total_input_tokens: info.input_tokens,
                total_output_tokens: info.output_tokens,
                status,
                pid: Some(live.pid),
                last_activity: info.last_activity,
                started_at: live.started_at,
                jsonl_path: path,
                last_file_size: info.file_size,
            });
        }
    }

    // Handle live sessions with no direct JSONL name match.
    // This covers two cases:
    //   1. Brand-new sessions (no JSONL yet) → show as New placeholder
    //   2. Resumed sessions (claude --resume creates a new session-id in the session file
    //      but continues appending to the original JSONL) → find via lsof, show real data
    let known_tmux: std::collections::HashSet<String> = sessions
        .iter()
        .filter_map(|s| s.tmux_session.clone())
        .collect();

    for (session_id_key, live) in &live_map {
        if known_tmux.contains(&live.tmux_session) {
            continue;
        }

        // For sessions that have a real session-id (not the "tmux-{name}" placeholder),
        // try to find the JSONL via lsof. This handles resumed sessions where the
        // session file's session-id doesn't match the original JSONL filename.
        let jsonl_path = if !session_id_key.starts_with("tmux-") {
            // Check prev_sessions cache first to avoid repeated ps calls
            let cached = prev_sessions
                .get(session_id_key.as_str())
                .filter(|s| !s.jsonl_path.as_os_str().is_empty())
                .map(|s| s.jsonl_path.clone());
            cached.or_else(|| find_jsonl_for_resumed_session(&live.tmux_session, live.pid))
        } else {
            None
        };

        if let Some(path) = jsonl_path {
            let prev = prev_sessions.get(session_id_key.as_str());
            let info = parse_jsonl(
                &path,
                prev.map(|s| s.last_file_size).unwrap_or(0),
                prev.map(|s| s.total_input_tokens).unwrap_or(0),
                prev.map(|s| s.total_output_tokens).unwrap_or(0),
                prev.and_then(|s| s.model.clone()),
                prev.and_then(|s| s.last_activity.clone()),
            );

            let cwd = info.cwd.clone().unwrap_or_else(|| live.pane_cwd.clone());
            let (project_name, branch) = git_project_info(&cwd);

            let status = determine_status(
                &path,
                info.input_tokens,
                info.output_tokens,
                Some(&live.tmux_session),
            );

            sessions.push(Session {
                session_id: session_id_key.clone(),
                project_name,
                branch,
                cwd,
                tmux_session: Some(live.tmux_session.clone()),
                model: info.model,
                total_input_tokens: info.input_tokens,
                total_output_tokens: info.output_tokens,
                status,
                pid: Some(live.pid),
                last_activity: info.last_activity,
                started_at: live.started_at,
                jsonl_path: path,
                last_file_size: info.file_size,
            });
        } else {
            // No JSONL found — brand-new session, show as New placeholder
            let (project_name, branch) = git_project_info(&live.pane_cwd);
            sessions.push(Session {
                session_id: session_id_key.clone(),
                project_name,
                branch,
                cwd: live.pane_cwd.clone(),
                tmux_session: Some(live.tmux_session.clone()),
                model: None,
                total_input_tokens: 0,
                total_output_tokens: 0,
                status: SessionStatus::New,
                pid: Some(live.pid),
                last_activity: None,
                started_at: live.started_at,
                jsonl_path: PathBuf::new(),
                last_file_size: 0,
            });
        }
    }

    // Sort by creation time (oldest first)
    sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
    sessions
}

/// Info about a live claude session, built from tmux + session files.
struct LiveSessionInfo {
    pid: i32,
    tmux_session: String,
    pane_cwd: String,
    started_at: u64,
}

/// Build a map from JSONL session_id → live session info.
///
/// Joins two sources:
///   1. tmux list-panes: PID → (tmux_session, pane_cwd) for panes running claude
///   2. ~/.claude/sessions/{PID}.json: PID → (session_id, started_at)
fn build_live_session_map() -> HashMap<String, LiveSessionInfo> {
    let pid_session_map = read_pid_session_map();
    let tmux_panes = discover_claude_tmux_panes();

    let mut map = HashMap::new();
    for (pid, tmux_session, pane_cwd) in tmux_panes {
        if let Some(info) = pid_session_map.get(&pid) {
            map.insert(
                info.session_id.clone(),
                LiveSessionInfo {
                    pid,
                    tmux_session,
                    pane_cwd,
                    started_at: info.started_at,
                },
            );
        } else {
            // Tmux pane running claude but no session file yet (just started).
            // Use the tmux session name as a placeholder key.
            map.insert(
                format!("tmux-{tmux_session}"),
                LiveSessionInfo {
                    pid,
                    tmux_session,
                    pane_cwd,
                    started_at: 0,
                },
            );
        }
    }
    map
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

use std::sync::Mutex;
use std::time::Instant;

struct GitInfo {
    repo_name: String,
    branch: Option<String>,
    fetched_at: Instant,
}

static GIT_CACHE: Mutex<Option<HashMap<String, GitInfo>>> = Mutex::new(None);

const GIT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Get the git project name and branch for a directory (cached for 30s).
fn git_project_info(cwd: &str) -> (String, Option<String>) {
    {
        let cache = GIT_CACHE.lock().unwrap();
        if let Some(info) = cache.as_ref().and_then(|c| c.get(cwd)) {
            if info.fetched_at.elapsed() < GIT_CACHE_TTL {
                return (info.repo_name.clone(), info.branch.clone());
            }
        }
    }

    let repo_name = fetch_git_repo_name(cwd);
    let branch = fetch_git_branch(cwd);

    let mut cache = GIT_CACHE.lock().unwrap();
    if cache.is_none() {
        *cache = Some(HashMap::new());
    }
    cache.as_mut().unwrap().insert(
        cwd.to_string(),
        GitInfo {
            repo_name: repo_name.clone(),
            branch: branch.clone(),
            fetched_at: Instant::now(),
        },
    );
    (repo_name, branch)
}

fn fetch_git_repo_name(cwd: &str) -> String {
    match std::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(o) if o.status.success() => {
            let toplevel = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Path::new(&toplevel)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| cwd.to_string())
        }
        _ => Path::new(cwd)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| cwd.to_string()),
    }
}

fn fetch_git_branch(cwd: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch)
    }
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
    prev_activity: Option<String>,
) -> ParsedInfo {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            return ParsedInfo {
                input_tokens: prev_input,
                output_tokens: prev_output,
                model: prev_model,
                cwd: None,
                last_activity: prev_activity,
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
            last_activity: prev_activity,
            status: SessionStatus::Idle,
            file_size,
        };
    }

    let mut reader = BufReader::new(file);
    let mut total_input = prev_input;
    let mut total_output = prev_output;
    let mut model = prev_model;
    let mut last_activity = prev_activity;
    let mut cwd = None;

    if prev_file_size > 0 {
        let _ = reader.seek(SeekFrom::Start(prev_file_size));
    } else {
        total_input = 0;
        total_output = 0;
        model = None;
        last_activity = None;
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

/// For a resumed session, find the original JSONL by locating the session-id
/// that `claude --resume` was called with.
///
/// `claude --resume <orig-id>` writes a new session-id to its session file but
/// continues appending to the original JSONL (named after the old session-id).
///
/// Strategy (in order):
///  1. Read `RECON_RESUMED_FROM` from the tmux session environment — set by
///     `recon --resume` at session creation time. Reliable and zero-overhead.
///  2. Fall back to parsing `ps` args for sessions started outside of recon
///     (e.g. the user ran `claude --resume <id>` in a tmux session manually).
fn find_jsonl_for_resumed_session(tmux_session: &str, pid: i32) -> Option<PathBuf> {
    // Try tmux environment variable first (set by recon --resume)
    let original_id = read_tmux_env(tmux_session, "RECON_RESUMED_FROM")
        // Fall back to parsing ps args
        .or_else(|| parse_resume_id_from_ps(pid))?;

    find_jsonl_by_session_id(&original_id)
}

/// Read a variable from a tmux session's environment table.
fn read_tmux_env(session_name: &str, var: &str) -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args(["show-environment", "-t", session_name, var])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    // Output format: "VAR=value\n"
    let line = String::from_utf8_lossy(&output.stdout);
    line.trim().split_once('=').map(|(_, v)| v.to_string())
}

/// Parse `--resume <session-id>` from the process command line via ps.
/// Fallback for sessions not created by `recon --resume`.
fn parse_resume_id_from_ps(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()?;

    let args = String::from_utf8_lossy(&output.stdout);
    args.trim()
        .split_whitespace()
        .skip_while(|&a| a != "--resume")
        .nth(1)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Find the JSONL file for a given session-id by scanning all project directories.
fn find_jsonl_by_session_id(session_id: &str) -> Option<PathBuf> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    for entry in fs::read_dir(&projects_dir).ok()?.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Find the cwd used by an existing session (by scanning its JSONL for a cwd entry).
/// Used by the resume command to start the tmux session in the right directory.
/// Return session-id → tmux info for all currently live claude sessions.
/// Used by the resume picker to filter out still-running sessions.
pub fn build_live_session_map_public() -> HashMap<String, String> {
    build_live_session_map()
        .into_iter()
        .map(|(id, info)| (id, info.tmux_session))
        .collect()
}

pub fn find_session_cwd(session_id: &str) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    for entry in fs::read_dir(&projects_dir).ok()?.flatten() {
        let jsonl_path = entry.path().join(format!("{session_id}.jsonl"));
        if !jsonl_path.exists() {
            continue;
        }
        let file = fs::File::open(&jsonl_path).ok()?;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines().take(20).flatten() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                    return Some(cwd.to_string());
                }
            }
        }
    }
    None
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

/// Determine status by inspecting the Claude Code TUI status bar.
///
/// The last non-empty line in the pane is always the status bar:
///   "esc to interrupt"  → agent is streaming or running a tool
///   "Esc to cancel"     → permission prompt waiting for user input
///   anything else       → idle, waiting for user input
fn pane_status(session_name: &str) -> SessionStatus {
    let output = match std::process::Command::new("tmux")
        .args(["capture-pane", "-t", session_name, "-p"])
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
        if trimmed.contains("esc to interrupt") {
            return SessionStatus::Working;
        }
        if trimmed.contains("Esc to cancel") {
            return SessionStatus::Input;
        }
        return SessionStatus::Idle;
    }

    SessionStatus::Idle
}

// --- Live session discovery ---

struct SessionFileInfo {
    session_id: String,
    started_at: u64,
}

/// Read ~/.claude/sessions/{PID}.json files to build a PID → session info map.
fn read_pid_session_map() -> HashMap<i32, SessionFileInfo> {
    let sessions_dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("sessions"),
        None => return HashMap::new(),
    };

    let entries = match fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return HashMap::new(),
    };

    let mut map = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let (Some(pid), Some(sid)) = (
                        v.get("pid").and_then(|p| p.as_i64()),
                        v.get("sessionId").and_then(|s| s.as_str()),
                    ) {
                        let started_at = v
                            .get("startedAt")
                            .and_then(|s| s.as_u64())
                            .unwrap_or(0);
                        map.insert(
                            pid as i32,
                            SessionFileInfo {
                                session_id: sid.to_string(),
                                started_at,
                            },
                        );
                    }
                }
            }
        }
    }
    map
}

/// Get tmux panes running claude.
/// Returns Vec<(pid, session_name, pane_cwd)>.
fn discover_claude_tmux_panes() -> Vec<(i32, String, String)> {
    let output = match std::process::Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_pid}\t#{session_name}\t#{pane_current_command}\t#{pane_current_path}",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let pid: i32 = match parts[0].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let session_name = parts[1];
        let command = parts[2];
        let pane_path = parts[3];

        // Claude shows up as a version number (e.g. "2.1.76") or "claude" or "node"
        let is_claude = command
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
            || command == "claude"
            || command == "node";

        if is_claude {
            results.push((pid, session_name.to_string(), pane_path.to_string()));
        } else if command == "bash" || command == "sh" || command == "zsh" {
            // The pane may be running a wrapper script with claude as a child.
            // Check if any child process has a session file.
            if let Some(claude_pid) = find_claude_child_pid(pid) {
                results.push((claude_pid, session_name.to_string(), pane_path.to_string()));
            }
        }
    }

    results
}

/// Check if a shell process has a claude child by looking for a child PID
/// that has a corresponding ~/.claude/sessions/{PID}.json file.
fn find_claude_child_pid(parent_pid: i32) -> Option<i32> {
    let sessions_dir = dirs::home_dir()?.join(".claude").join("sessions");
    let output = std::process::Command::new("pgrep")
        .args(["-P", &parent_pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .find(|pid| sessions_dir.join(format!("{pid}.json")).exists())
}

