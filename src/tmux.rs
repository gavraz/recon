use std::process::Command;

use crate::session;

/// Switch to a tmux session (inside tmux) or attach to it (outside tmux).
pub fn switch_to_session(name: &str) {
    let inside_tmux = std::env::var("TMUX").is_ok();
    if inside_tmux {
        let _ = Command::new("tmux")
            .args(["switch-client", "-t", name])
            .status();
    } else {
        let _ = Command::new("tmux")
            .args(["attach-session", "-t", name])
            .status();
    }
}

/// Launch claude in a new tmux session with the given name and working directory.
/// Returns the session name on success.
pub fn create_session(name: &str, cwd: &str) -> Result<String, String> {
    let base_name = sanitize_session_name(name);
    let session_name = unique_session_name(&base_name);

    let claude_path = which_claude().unwrap_or_else(|| "claude".to_string());
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-c",
            cwd,
            &claude_path,
        ])
        .status()
        .map_err(|e| format!("Failed to create tmux session: {e}"))?;

    if !status.success() {
        return Err("tmux new-session failed".to_string());
    }

    Ok(session_name)
}

/// Resume a claude session in a new tmux session.
/// No-op if the session is already running — returns the existing tmux name.
pub fn resume_session(session_id: &str, name: Option<&str>) -> Result<String, String> {
    if let Some(existing) = session::find_live_tmux_for_session(session_id) {
        return Ok(existing);
    }

    let tmux_name = name
        .map(|n| n.to_string())
        .unwrap_or_else(|| session_id[..6.min(session_id.len())].to_string());

    // Use the original session's cwd so we start in the right project directory.
    let cwd = session::find_session_cwd(session_id)
        .or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().to_string()).ok())
        .unwrap_or_else(|| ".".to_string());

    let base_name = sanitize_session_name(&tmux_name);
    let session_name = unique_session_name(&base_name);

    let claude_path = which_claude().unwrap_or_else(|| "claude".to_string());
    // Store the original session-id in the tmux session environment so recon can
    // find the right JSONL without parsing process command lines.
    let env_var = format!("RECON_RESUMED_FROM={session_id}");
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-c",
            &cwd,
            "-e",
            &env_var,
            &claude_path,
            "--resume",
            session_id,
        ])
        .status()
        .map_err(|e| format!("Failed to create tmux session: {e}"))?;

    if !status.success() {
        return Err("tmux new-session failed".to_string());
    }

    Ok(session_name)
}

/// Get default session name and cwd for a new session.
pub fn default_new_session_info() -> (String, String) {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let name = std::path::Path::new(&cwd)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "claude".to_string());

    (name, cwd)
}

fn unique_session_name(base_name: &str) -> String {
    if !session_exists(base_name) {
        return base_name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base_name}-{n}");
        if !session_exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn which_claude() -> Option<String> {
    let output = Command::new("which").arg("claude").output().ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() { None } else { Some(path) }
}

/// Kill a tmux session by name.
pub fn kill_session(name: &str) -> bool {
    Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Sanitize a string for use as a tmux session name.
/// Strips characters that are special in tmux target specifications
/// (dots, colons, equals, dollar, exclamation, percent, at, spaces)
/// and removes control characters.
fn sanitize_session_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '.' | ':' | '=' | '$' | '!' | '%' | '@' | ' ' | '\t' => '-',
            _ => c,
        })
        .collect();
    // Ensure the name is not empty after sanitization
    if sanitized.is_empty() {
        "session".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_dots_and_colons() {
        assert_eq!(sanitize_session_name("my.app:v2"), "my-app-v2");
    }

    #[test]
    fn sanitize_replaces_tmux_special_chars() {
        assert_eq!(sanitize_session_name("te$t!app@home"), "te-t-app-home");
        assert_eq!(sanitize_session_name("a=b%c"), "a-b-c");
    }

    #[test]
    fn sanitize_replaces_spaces() {
        assert_eq!(sanitize_session_name("my project"), "my-project");
        // \t is a control character, so it gets stripped entirely
        assert_eq!(sanitize_session_name("a\tb"), "ab");
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(sanitize_session_name("test\x00name\x1b"), "testname");
    }

    #[test]
    fn sanitize_empty_string_returns_session() {
        assert_eq!(sanitize_session_name(""), "session");
    }

    #[test]
    fn sanitize_all_special_chars_returns_session() {
        // All chars map to '-', but dashes are kept, so not empty
        assert_eq!(sanitize_session_name(".:"), "--");
    }

    #[test]
    fn sanitize_preserves_normal_names() {
        assert_eq!(sanitize_session_name("my-project-123"), "my-project-123");
        assert_eq!(sanitize_session_name("api-refactor"), "api-refactor");
    }
}
