use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
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
    pub effort: Option<String>,
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
        format!("{}k / {}", used / 1000, format_window(window))
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

    pub fn model_display(&self) -> String {
        match &self.model {
            Some(m) => model::format_with_effort(m, self.effort.as_deref().unwrap_or("")),
            None => "—".to_string(),
        }
    }
}

pub fn format_window(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}k", tokens / 1000)
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
                prev.and_then(|s| s.effort.clone()),
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
                effort: info.effort,
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

    // Post-scan fixup: /clear creates a new JSONL without updating {PID}.json,
    // so the first scan matches the OLD JSONL while the new one is ignored.
    // Detect /clear-born JONLs (they have <command-name>/clear</command-name>
    // in their first few lines) and switch matched sessions to the newest one.
    for session in &mut sessions {
        if let Some(newer) =
            find_clear_successor(&session.cwd, &matched_session_ids, &session.jsonl_path)
        {
            let info = parse_jsonl(&newer, 0, 0, 0, None, None, None);
            let new_sid = newer
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            matched_session_ids.remove(&session.session_id);
            matched_session_ids.insert(new_sid);
            session.total_input_tokens = info.input_tokens;
            session.total_output_tokens = info.output_tokens;
            session.model = info.model;
            session.effort = info.effort;
            session.last_activity = info.last_activity;
            session.last_file_size = info.file_size;
            session.jsonl_path = newer;
            if let Some(cwd) = info.cwd {
                session.cwd = cwd;
            }
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
        // try to find the JSONL via resume detection. This handles resumed sessions
        // where the session file's session-id doesn't match the original JSONL filename.
        //
        // However, if the session was /reset after being resumed, the ps args still
        // show the old --resume ID while a new JSONL exists. In that case, the resume
        // JSONL is stale. We detect this: if the resumed JSONL's session-id matches
        // the session_id_key (from {PID}.json), the resume is current; otherwise
        // /reset happened and we skip the stale resume path.
        let jsonl_path = if !session_id_key.starts_with("tmux-") {
            let cached = prev_sessions
                .get(session_id_key.as_str())
                .filter(|s| !s.jsonl_path.as_os_str().is_empty())
                .map(|s| s.jsonl_path.clone());
            let resume_path = cached.or_else(|| find_jsonl_for_resumed_session(&live.tmux_session, live.pid));
            // Skip the resume path if a /clear successor exists in the same
            // project dir — the resumed JSONL is stale, /clear created a new one.
            resume_path.filter(|p| {
                find_clear_successor(&live.pane_cwd, &matched_session_ids, p).is_none()
            })
        } else {
            None
        };

        // Try resumed session JSONL, then /reset fallback, then New placeholder
        let resolved_path = jsonl_path
            .or_else(|| find_recent_unmatched_jsonl(&live.pane_cwd, &matched_session_ids));

        // Mark as claimed so other sessions in the same dir don't grab the same JSONL
        if let Some(ref path) = resolved_path {
            if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                matched_session_ids.insert(stem);
            }
        }

        if let Some(path) = resolved_path {
            let prev = prev_sessions.get(session_id_key.as_str());
            let info = parse_jsonl(
                &path,
                prev.map(|s| s.last_file_size).unwrap_or(0),
                prev.map(|s| s.total_input_tokens).unwrap_or(0),
                prev.map(|s| s.total_output_tokens).unwrap_or(0),
                prev.and_then(|s| s.model.clone()),
                prev.and_then(|s| s.effort.clone()),
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
                effort: info.effort,
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
                effort: None,
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

    // Sort by last activity (most recent first), sessions with no activity last
    sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
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
    effort: Option<String>,
    cwd: Option<String>,
    last_activity: Option<String>,
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

/// Validate that a CWD path is safe to pass to external commands.
/// Rejects paths that are not absolute or that resolve (via symlinks) to
/// a location outside the original directory tree.
fn is_safe_cwd(cwd: &str) -> bool {
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return false;
    }
    path.is_dir()
}

fn fetch_git_repo_name(cwd: &str) -> String {
    if !is_safe_cwd(cwd) {
        return Path::new(cwd)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| cwd.to_string());
    }
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
    if !is_safe_cwd(cwd) {
        return None;
    }
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
    message: Option<MessageEntry>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
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
    prev_effort: Option<String>,
    prev_activity: Option<String>,
) -> ParsedInfo {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            return ParsedInfo {
                input_tokens: prev_input,
                output_tokens: prev_output,
                model: prev_model,
                effort: prev_effort,
                cwd: None,
                last_activity: prev_activity,
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
            effort: prev_effort,
            cwd: None,
            last_activity: prev_activity,
            file_size,
        };
    }

    let mut reader = BufReader::new(file);
    let mut total_input = prev_input;
    let mut total_output = prev_output;
    let mut model = prev_model;
    let mut effort = prev_effort;
    let mut last_activity = prev_activity;
    let mut cwd = None;

    if prev_file_size > 0 {
        let _ = reader.seek(SeekFrom::Start(prev_file_size));
    } else {
        total_input = 0;
        total_output = 0;
        model = None;
        effort = None;
        last_activity = None;
    }

    const MAX_LINE_LEN: usize = 10 * 1024 * 1024; // 10 MB per line
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        // Skip lines that are unreasonably large to prevent memory exhaustion
        if line.len() > MAX_LINE_LEN {
            continue;
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
            // Extract model + effort from /model command stdout recorded in JSONL:
            //   "Set model to Opus 4.6 (1M context) (default) with max effort"
            //   "Set model to Sonnet 4.6"
            if trimmed.contains("<local-command-stdout>Set model to")
                && !trimmed.contains("toolUseResult")
                && !trimmed.contains("tool_result")
            {
                let stdout_pos = trimmed.find("<local-command-stdout>Set model to").unwrap();
                let tag_end = stdout_pos + "<local-command-stdout>Set model to".len();
                let raw_remainder = &trimmed[tag_end..];
                // Truncate at closing tag
                let raw_remainder = raw_remainder
                    .find("</local-command-stdout>")
                    .map_or(raw_remainder, |end| &raw_remainder[..end]);
                let remainder = strip_ansi(raw_remainder);
                let remainder = remainder.trim();

                // Extract effort if present ("with <effort> effort")
                let (model_part, new_effort) = if let Some(wp) = remainder.find("with ") {
                    let after_with = &remainder[wp + 5..];
                    let eff = after_with.find(" effort")
                        .map(|end| after_with[..end].trim().to_string())
                        .filter(|s| !s.is_empty());
                    (&remainder[..wp], eff)
                } else {
                    (&remainder[..], None)
                };
                if let Some(e) = new_effort {
                    effort = Some(e);
                }

                // Extract model: strip suffixes like "(1M context)" and "(default)"
                let model_name = model_part
                    .trim()
                    .trim_end_matches("(default)")
                    .trim()
                    .trim_end_matches("(1M context)")
                    .trim()
                    .trim_end_matches("(200k context)")
                    .trim();
                if let Some(id) = model::id_from_display_name(model_name) {
                    model = Some(id.to_string());
                }
            }
        }
    }

    ParsedInfo {
        input_tokens: total_input,
        output_tokens: total_output,
        model,
        effort,
        cwd,
        last_activity,
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

/// Strip ANSI escape sequences from a string.
/// Handles both raw ESC byte (\x1b[...m) and JSON-encoded form (\\u001b[...m).
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Raw ESC byte: skip until 'm'
            for next in chars.by_ref() {
                if next == 'm' { break; }
            }
        } else if c == '\\' && chars.peek() == Some(&'u') {
            // Check for JSON-escaped \\u001b
            let rest: String = chars.clone().take(5).collect();
            if rest.starts_with("u001b") || rest.starts_with("u001B") {
                // Consume "u001b" (5 chars)
                for _ in 0..5 { chars.next(); }
                // Skip the ANSI parameter sequence until 'm'
                for next in chars.by_ref() {
                    if next == 'm' { break; }
                }
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Encode a CWD path to a Claude project directory name.
/// Claude Code replaces both `/` and `.` with `-`.
fn encode_project_path(cwd: &str) -> String {
    cwd.replace('/', "-").replace('.', "-")
}

/// Find the newest unmatched JSONL in the project directory for `cwd` that was
/// created by `/clear` (has the `/clear` command marker in its first few lines)
/// and is newer than `current_jsonl`. Returns None if no such file exists.
fn find_clear_successor(
    cwd: &str,
    matched_session_ids: &std::collections::HashSet<String>,
    current_jsonl: &Path,
) -> Option<PathBuf> {
    let cur_mtime = current_jsonl.metadata().ok().and_then(|m| m.modified().ok())?;
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    let project_dir = projects_dir.join(encode_project_path(cwd));

    if !project_dir.is_dir() {
        return None;
    }

    let mut best: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(&project_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            continue;
        }
        let session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if matched_session_ids.contains(&session_id) {
            continue;
        }
        let modified = match path.metadata().ok().and_then(|m| m.modified().ok()) {
            Some(t) => t,
            None => continue,
        };
        // Must be newer than the current JSONL
        if modified <= cur_mtime {
            continue;
        }
        // Check for /clear marker in first few lines
        if !is_clear_born(&path) {
            continue;
        }
        if best.as_ref().map_or(true, |(_, t)| modified > *t) {
            best = Some((path, modified));
        }
    }
    best.map(|(p, _)| p)
}

/// Check if a JSONL file was created by `/clear` by looking for the command
/// marker in its first few lines.
fn is_clear_born(path: &Path) -> bool {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let reader = BufReader::new(file);
    let mut line = String::new();
    let mut reader = reader;
    for _ in 0..5 {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.contains("<command-name>/clear</command-name>") {
            return true;
        }
    }
    false
}

/// Find the most recently modified JSONL in the project directory for `cwd`
/// whose session ID is not already claimed by another live session.
///
/// This handles `/reset` (and `/clear`): Claude Code creates a new session ID
/// and JSONL but does NOT update `{PID}.json`, so the live map points at a
/// stale session ID with no matching JSONL file.
fn find_recent_unmatched_jsonl(
    cwd: &str,
    matched_session_ids: &std::collections::HashSet<String>,
) -> Option<PathBuf> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    let project_dir = projects_dir.join(encode_project_path(cwd));

    if !project_dir.is_dir() {
        return None;
    }

    let mut best: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(&project_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            continue;
        }
        let session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if matched_session_ids.contains(&session_id) {
            continue;
        }
        let modified = match path.metadata().ok().and_then(|m| m.modified().ok()) {
            Some(t) => t,
            None => continue,
        };
        if best.as_ref().map_or(true, |(_, t)| modified > *t) {
            best = Some((path, modified));
        }
    }
    best.map(|(p, _)| p)
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

/// Check if a session ID (JSONL-based) is already running in tmux.
/// Returns the tmux session name if found.
pub fn find_live_tmux_for_session(session_id: &str) -> Option<String> {
    let live_map = build_live_session_map();

    // Direct match: PID file's session_id == the one we're looking for.
    if let Some(info) = live_map.get(session_id) {
        return Some(info.tmux_session.clone());
    }

    // Resumed session: RECON_RESUMED_FROM env var matches.
    for (_, info) in &live_map {
        if let Some(orig_id) = read_tmux_env(&info.tmux_session, "RECON_RESUMED_FROM") {
            if orig_id == session_id {
                return Some(info.tmux_session.clone());
            }
        }
    }

    None
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
/// The last non-empty line in the pane is typically the status bar:
///   "esc to interrupt"  → agent is streaming or running a tool
///   "Esc to cancel"     → permission prompt waiting for user input
///
/// Some permission prompts use a selection menu instead of "Esc to cancel"
/// (e.g. fetch permissions). We scan a few lines back to detect those via
/// the "❯ N." selection arrow pattern (e.g. "❯ 2. Yes, and don't ask again").
fn pane_status(session_name: &str) -> SessionStatus {
    let output = match std::process::Command::new("tmux")
        .args(["capture-pane", "-t", session_name, "-p"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return SessionStatus::Idle,
    };

    let content = String::from_utf8_lossy(&output.stdout);

    let mut lines_checked = 0;
    for line in content.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Status bar is always the very last non-empty line
        if lines_checked == 0 {
            if trimmed.contains("esc to interrupt") {
                return SessionStatus::Working;
            }
            if trimmed.contains("Esc to cancel") {
                return SessionStatus::Input;
            }
        }

        // Selection-style permission prompts show "❯ N." (arrow + digit)
        // e.g. " ❯ 2. Yes, and don't ask again for docs.asciinema.org"
        // This is distinct from the regular prompt "❯ user text".
        if let Some(pos) = trimmed.find('❯') {
            let after = trimmed[pos + '❯'.len_utf8()..].trim_start();
            if after.starts_with(|c: char| c.is_ascii_digit()) {
                return SessionStatus::Input;
            }
        }

        lines_checked += 1;
        if lines_checked >= 5 {
            break;
        }
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
            "#{pane_pid}|||#{session_name}|||#{pane_current_command}|||#{pane_current_path}",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let sessions_dir = dirs::home_dir()
        .map(|h| h.join(".claude").join("sessions"))
        .unwrap_or_default();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, "|||").collect();
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
            // pane_pid is the initial process — it may be claude itself (recon launch)
            // or a shell with claude as the foreground child (manual `claude` in a terminal).
            // Try the pane PID first, fall back to searching children.
            let claude_pid = if sessions_dir.join(format!("{pid}.json")).exists() {
                Some(pid)
            } else {
                find_claude_child_pid(pid)
            };
            if let Some(cpid) = claude_pid {
                results.push((cpid, session_name.to_string(), pane_path.to_string()));
            }
        } else if command == "bash" || command == "sh" || command == "zsh" {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_safe_cwd_rejects_relative_paths() {
        assert!(!is_safe_cwd("relative/path"));
        assert!(!is_safe_cwd("./here"));
        assert!(!is_safe_cwd("../parent"));
    }

    #[test]
    fn is_safe_cwd_rejects_nonexistent_absolute_paths() {
        assert!(!is_safe_cwd("/nonexistent/path/that/does/not/exist/xyz123"));
    }

    #[test]
    fn is_safe_cwd_accepts_real_directories() {
        assert!(is_safe_cwd("/tmp"));
        // Home directory should always exist
        if let Some(home) = dirs::home_dir() {
            assert!(is_safe_cwd(&home.to_string_lossy()));
        }
    }

    #[test]
    fn fetch_git_repo_name_handles_nonexistent_cwd() {
        // Should return the basename, not panic or invoke git
        let result = fetch_git_repo_name("/nonexistent/path/my-project");
        assert_eq!(result, "my-project");
    }

    #[test]
    fn fetch_git_branch_handles_nonexistent_cwd() {
        let result = fetch_git_branch("/nonexistent/path/my-project");
        assert!(result.is_none());
    }
}

