# recon

A tmux-native dashboard for managing [Claude Code](https://claude.ai/claude-code) agents.

Run multiple Claude Code sessions in tmux, then manage them all without ever leaving the terminal — see what each agent is working on, which ones need your attention, switch between them, kill or spawn new ones, and resume past sessions. All from a single keybinding.

![recon demo](assets/demo.gif)

## Views

### Tamagotchi View (`recon view` or press `v`)

A visual dashboard where each agent is a pixel-art creature living in a room. Designed for a side monitor — glance over and instantly see who's working, sleeping, or needs attention.

Creatures are rendered as colored pixel art using half-block characters. Working and Input creatures animate; Idle and New stay still.

| State | Creature | Color |
|-------|----------|-------|
| **Working** | Happy blob with sparkles and feet | Green |
| **Input** | Angry blob with furrowed brows | Orange (pulsing) |
| **Idle** | Sleeping blob with Zzz | Blue-grey |
| **New** | Egg with spots | Cream |

- **Rooms** group agents by git repository — worktrees of the same repo share a room, while monorepo sub-projects get their own (e.g. `myapp` vs `myapp › tools/cli`) (2×2 grid, paginated)
- **Zoom** into a room with `1`-`4`, page with `j`/`k`
- **Context bar** per agent with green/yellow/red coloring

### Table View (default)

```
┌─ recon — Claude Code Sessions ──────────────────────────────────────────────────────────────────────────┐
│  #  Session          Git(Project::Branch)   Directory          Status  Model       Context  Last Active │
│  1  api-refactor     myapp::feat/auth       ~/repos/myapp      ● Input Opus 4.6    45k/1M   2m ago      │
│  2  debug-pipeline   infra::main            ~/repos/infra      ● Work  Sonnet 4.6  12k/200k < 1m        │
│  3  write-tests      myapp::feat/auth       ~/repos/myapp      ● Work  Haiku 4.5   8k/200k  < 1m        │
│  4  code-review      webapp::pr-452         ~/repos/webapp     ● Idle  Sonnet 4.6  90k/200k 5m ago      │
│  5  scratch          recon::main            ~/repos/recon      ● Idle  Opus 4.6    3k/1M    10m ago     │
│  6  new-session      dotfiles::main         ~/repos/dotfiles   ● New   —           —        —           │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
j/k navigate  Enter switch  / search  v view  q quit
```

- **Input** rows are highlighted — these sessions are blocked waiting for your approval
- **Working** sessions are actively streaming or running tools
- **Idle** sessions are done and waiting for your next prompt
- **New** sessions haven't had any interaction yet

## How it works

recon is built around **tmux**. Each Claude Code instance runs in its own tmux session.

```
┌─────────────────────────────────────────────────────────┐
│                      tmux server                        │
│                                                         │
│  ┌───────────────┐  ┌───────────────┐  ┌──────────────┐ │
│  │ session:      │  │ session:      │  │ session:     │ │
│  │ api-refactor  │  │ debug-pipe    │  │ scratch      │ │
│  │               │  │               │  │              │ │
│  │  ┌──────────┐ │  │  ┌──────────┐ │  │  ┌────────┐  │ │
│  │  │  claude  │ │  │  │  claude  │ │  │  │ claude │  │ │
│  │  └──────────┘ │  │  └──────────┘ │  │  └────────┘  │ │
│  └───────┬───────┘  └───────┬───────┘  └───────┬──────┘ │
│          │                  │                  │        │
└──────────┼──────────────────┼──────────────────┼────────┘
           │                  │                  │
           ▼                  ▼                  ▼
     ┌──────────────────────────────────────────────┐
     │                 recon (TUI)                   │
     │                                               │
     │  reads:                                       │
     │   • tmux list-panes → PID, session name       │
     │   • ~/.claude/sessions/{PID}.json             │
     │   • ~/.claude/projects/…/*.jsonl              │
     │   • tmux capture-pane → status bar text       │
     └──────────────────────────────────────────────┘
```

**Status detection** inspects the Claude Code TUI status bar at the bottom of each tmux pane:

| Status bar text | State |
|---|---|
| `esc to interrupt` | **Working** — streaming response or running a tool |
| `Esc to cancel` | **Input** — permission prompt, waiting for you |
| anything else | **Idle** — waiting for your next prompt |
| *(0 tokens)* | **New** — no interaction yet |

**Session matching** uses `~/.claude/sessions/{PID}.json` files that Claude Code writes, linking each process to its session ID. No `ps` parsing or CWD-based heuristics.

## Install

```bash
cargo install --path .
```

Requires tmux and [Claude Code](https://claude.ai/claude-code).

## Usage

```bash
recon                                        # Table dashboard
recon view                                   # Tamagotchi visual dashboard
recon json                                   # JSON output (for scripting)
recon launch                                 # Create a new claude session (background)
recon launch --name foo --cwd ~/repos/myapp  # Custom name and directory
recon launch --command "claude --model sonnet" --attach  # Custom command, attach to session
recon launch --tag env:staging --tag role:reviewer       # Tag a session (key:value metadata)
recon json --tag role:reviewer               # Filter JSON output by tag (must match all)
recon new                                    # Interactive new session form
recon resume                                 # Interactive resume picker
recon resume --id <session-id>               # Resume a specific session
recon resume --id <session-id> --name foo    # Resume with a custom tmux session name
recon next                                   # Jump to the next agent waiting for input
recon park                                   # Save all live sessions to disk
recon unpark                                 # Restore previously parked sessions
```

### Keybindings — Table View

| Key | Action |
|---|---|
| `j` / `k` | Navigate sessions |
| `Enter` | Switch to selected tmux session |
| `/` | Search / filter sessions by name |
| `i` / `Tab` | Jump to next agent waiting for input |
| `x` | Kill selected session |
| `v` | Switch to Tamagotchi view |
| `q` / `Esc` | Quit (Esc clears filter first) |

### Keybindings — Tamagotchi View

| Key | Action |
|---|---|
| `1`-`4` | Zoom into room |
| `/` | Search / filter sessions by name |
| `j` / `k` | Previous / next page |
| `h` / `l` | Select agent (when zoomed) |
| `Enter` | Switch to selected agent (when zoomed) |
| `x` | Kill selected agent (when zoomed) |
| `n` | New session in room (when zoomed) |
| `Esc` | Zoom out (or quit) |
| `v` | Switch to table view |
| `q` | Quit |

## Configuration

recon reads an optional config file from the platform config directory:

- Linux: `~/.config/recon/config.toml`
- macOS: `~/Library/Application Support/recon/config.toml`

With no file present, recon uses its built-in defaults. Today the config controls
which columns the table shows and in what order:

```toml
[table]
# Pick and order the columns. The leading `#` index column is always shown.
columns = ["session", "window", "project", "status", "context", "last_activity"]

# Optional per-column width overrides (in cells).
[table.widths]
window = 24
```

Run `recon config --available-columns` to list every column name (and print an
example stanza). A malformed config or an unknown column name fails fast with a
clear error on launch rather than silently falling back.

## tmux config

The included `tmux.conf` provides keybindings to open recon as a popup overlay:

```bash
# Add to your ~/.tmux.conf
bind g display-popup -E -w 80% -h 60% "recon"        # prefix + g → dashboard
bind n display-popup -E -w 80% -h 60% "recon new"    # prefix + n → new session
bind r display-popup -E -w 80% -h 60% "recon resume" # prefix + r → resume picker
bind i run-shell "recon next"                         # prefix + i → jump to next input agent
bind X confirm-before -p "Kill session #S? (y/n)" kill-session
```

This lets you pop open the dashboard from any tmux session, pick a session with `Enter`, and jump straight to it.

## Known Limitations

- **`/clear` resets session tracking** — Claude Code's `/clear` command creates a new JSONL file without updating the session-to-process mapping. After `/clear`, recon may show stale data (old tokens, old timestamps) until the session is restarted. Workaround: kill the session in recon and create a new one.

## Contribution Policy

This project is not accepting code contributions (Pull Requests) at this time.

Due to the sensitive nature of reconnaissance and session tracking, I prefer to maintain full control over the codebase to ensure security and auditability.

Ideas and feedback are welcome! Please open an [Issue](https://github.com/gavraz/recon/issues) if you have a feature request or have found a bug. If I like an idea, I will implement it myself.

## License

MIT
