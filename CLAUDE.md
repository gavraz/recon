# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```bash
cargo build                    # Debug build
cargo install --path .         # Install to ~/.cargo/bin/recon
./tests/e2e_states.sh          # E2E tests (creates real tmux sessions with claude)
```

The e2e tests require `jq`, `claude`, and a running tmux server. They create sessions with random IDs (`e2e-{RID}-*`) and clean up via trap on exit. Tests take ~2 minutes (waiting for claude to respond).

## Architecture

recon is a TUI dashboard that monitors Claude Code sessions running in tmux. It polls every 2 seconds.

### Data flow

```
tmux list-panes (#{pane_pid})  ──→  PID → tmux session name
~/.claude/sessions/{PID}.json  ──→  PID → JSONL session ID + startedAt
~/.claude/projects/*/*.jsonl   ──→  session ID → tokens, model, timestamps
tmux capture-pane              ──→  tmux session → status (last line of pane)
```

These four sources are joined in `session::discover_sessions()` via `build_live_session_map()`, which produces a `HashMap<session_id, LiveSessionInfo>` keyed by JSONL session ID. JSONL files are then matched against this map.

### Session matching

Process-to-session matching uses `~/.claude/sessions/{PID}.json` files written by Claude Code. This is the authoritative link — no CWD-based heuristics (which caused token-swapping bugs when multiple sessions shared a directory).

### Status detection

`pane_status()` reads the Claude Code TUI status bar (last non-empty line of tmux pane):
- `esc to interrupt` → Working
- `Esc to cancel` → Input
- anything else → Idle
- 0 tokens → New (checked before pane inspection)

### JSONL parsing

`parse_jsonl()` is incremental — it tracks `last_file_size` per session and seeks to that offset on subsequent reads, carrying forward tokens, model, and last_activity from previous state.

### Module roles

- **session.rs** — all discovery, parsing, and status logic (~660 lines, the core)
- **app.rs** — state container, refresh loop, key handling, JSON serialization
- **ui.rs** — ratatui rendering (table, status dots, color coding)
- **tmux.rs** — session creation, switching, name sanitization
- **model.rs** — model ID → display name/context window mapping. Constants are sourced from Anthropic's official models overview: https://platform.claude.com/docs/en/about-claude/models/overview.md (fetch the `.md` variant for an easy diff). When a new model ships or a context window is wrong, refresh from that page — see the module-level doc comment in `src/model.rs` for the last-synced table. Context-window size is per-model, not per-tier (e.g. Sonnet 4.6 is 1M but Sonnet 4.5 is 200k), so list each model explicitly.
- **new_session.rs** — interactive two-field form for creating sessions

### Key caches

- `GIT_REPO_CACHE` (static Mutex) — git repo root per CWD (doesn't change)
- `prev_sessions` (passed into `discover_sessions`) — previous file sizes and parsed values for incremental JSONL parsing

Git branch is intentionally NOT cached since it can change between refreshes.
