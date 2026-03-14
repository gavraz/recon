# recon view — Tamagotchi-Style Graphic Dashboard

## Vision

A graphical, always-on dashboard for monitoring Claude Code agents. Think Tamagotchi meets dev tools — each agent is a little creature living in a room, and you're raising them. The dashboard sits on a side monitor so you can glance over and instantly know: who's working, who's sleeping, and who's crying for attention.

```
recon view
```

Opens a TUI-based Tamagotchi dashboard using ratatui. No browser, no web server — runs in the same terminal environment as the table view. Toggle between table and view mode with `v`. Works alongside the existing TUI mode (`recon` for table, `recon view` for Tamagotchi).

## Prior Art

**Pixel Agents** (VS Code extension by Pablo De Lucca) pioneered the concept of rendering AI coding agents as pixel-art characters in a virtual office. It uses Canvas 2D + React in a VS Code webview, reading Claude Code's JSONL files.

Key differences from recon view:
- Pixel Agents has unreliable status detection (JSONL heuristics that "frequently misfire")
- recon has authoritative status via tmux pane text parsing
- Pixel Agents is VS Code-only; recon view is browser-based, terminal-native
- recon view uses the Tamagotchi metaphor instead of office workers

## The Tamagotchi Metaphor

Each Claude Code agent is a small creature you're "raising." The metaphor maps naturally to agent states and creates an emotional connection that makes monitoring feel engaging rather than tedious.

| Agent State | Creature Behavior | Visual |
|-------------|-------------------|--------|
| **New** | Egg hatching | Egg wobbles, cracks, creature emerges |
| **Working** | Happily active | Bouncing around, sparkles, building something |
| **Idle** | Sleeping/napping | Eyes closed, "Zzz" floating, curled up |
| **Input** | Hungry/crying | Tears, jumping, alert bubble with "!" |
| **High context** | Getting tired | Sweat drops, slower movement, panting |

The emotional hook: "are my little guys okay?" makes you want to check on them.

## Rooms

### Grouping Logic

Rooms group agents by **working directory basename**. Not by git repo (too coarse for monorepos), not custom names (too much friction initially).

```
/Users/gavra/repos/recon    → room "recon"
/Users/gavra/repos/api      → room "api"
/Users/gavra/repos/recon    → room "recon" (same room as first)
```

Multiple agents in the same CWD share a room. Rooms auto-create when a session appears and auto-destroy when the last session in that room disappears.

### Room Display

```
┌─ recon (2 agents) ──────────┐  ┌─ api (1 agent) ──────────────┐
│                              │  │                               │
│   😊        😴              │  │          😊                   │
│  "refactor" "tests"         │  │        "auth-flow"            │
│                              │  │                               │
└──────────────────────────────┘  └───────────────────────────────┘
```

- Room title = CWD basename + agent count
- Room border turns yellow/orange if any agent inside needs input
- Rooms arrange in a responsive grid that reflows with browser width
- Empty rooms fade out and disappear

### Future: Custom Room Names

A config file (`~/.config/recon/rooms.toml` or similar) could map CWD patterns to custom room names:

```toml
[rooms]
"/Users/gavra/repos/recon" = "HQ"
"/Users/gavra/repos/api-*" = "Backend"
```

Not in scope for Phase 1.

## Characters

### Identity

Each agent gets a deterministic character appearance based on a hash of its session ID. This means:
- The same session always looks the same across refreshes
- Different sessions are visually distinguishable
- No configuration needed

Character variation can come from: color palette, accessory (hat, glasses), body shape, or species.

### Info Display

**Always visible** (below character):
- Tmux session name (the creature's "name")

**On hover** (floating card):
```
┌─────────────────────────┐
│ main ← feat/auth        │  ← git branch
│ Opus 4.6 · 45k / 1M     │  ← model + context
│ 2m ago                   │  ← last activity
│ ████████░░ 80%           │  ← context bar
└─────────────────────────┘
```

- Context bar color: green (<75%) → yellow (75-90%) → red (>90%)

### Animations by State

**New (Egg)**:
- Static egg that wobbles periodically
- After first activity: crack animation, creature hatches

**Working (Happy)**:
- Character bounces lightly
- Small sparkle/star particles
- Optional: tiny hammer/wrench animation

**Idle (Sleeping)**:
- Character curled up or head down
- "Zzz" text floats upward and fades
- Muted/dimmed colors

**Input Needed (Hungry/Crying)**:
- Character jumps up and down urgently
- Pulsing yellow/orange glow around character
- Alert bubble with "!" above head
- Tears or sweat drops
- This is the most visually aggressive state — must be glanceable from across a room

**High Context Usage (Tired)**:
- Sweat drops appear when context > 75%
- Movement slows down when context > 90%
- Combines with other states (e.g., working + tired)

## Architecture

### Overview

```
┌──────────────────────────────────────────────────┐
│                    Browser                        │
│                                                   │
│  Canvas 2D renderer                               │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐            │
│  │  Room 1  │ │  Room 2  │ │  Room 3  │            │
│  │ 🐣 😊   │ │   😴    │ │ 😊 😊 😤 │            │
│  └─────────┘ └─────────┘ └─────────┘            │
│         ▲                                         │
│         │ SSE (Server-Sent Events, every 2s)      │
└─────────┼─────────────────────────────────────────┘
          │
┌─────────┼─────────────────────────────────────────┐
│  recon  │  Rust backend                            │
│         │                                          │
│  ┌──────┴──────┐    ┌──────────────────┐          │
│  │ axum server  │    │ discover_sessions │          │
│  │ /            │    │ (existing logic)  │          │
│  │ /events (SSE)│◄───│                  │          │
│  └─────────────┘    └──────────────────┘          │
│                             │                      │
│              ┌──────────────┼──────────────┐       │
│              ▼              ▼              ▼       │
│         tmux panes    JSONL files    session JSON  │
└────────────────────────────────────────────────────┘
```

### Data Flow

1. `discover_sessions()` runs every 2 seconds (existing logic, unchanged)
2. Sessions are serialized to JSON (existing `--json` output format)
3. axum SSE endpoint pushes the JSON to all connected browsers
4. Browser JS groups sessions into rooms by CWD basename
5. Canvas 2D renders rooms, characters, and animations

### Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| HTTP server | **axum** | Lightweight, async, already in tokio ecosystem |
| Data transport | **SSE** | Simpler than WebSocket; data flows one direction only |
| Frontend rendering | **Canvas 2D** | Proven for pixel art (Pixel Agents uses it), lightweight |
| Frontend framework | **Vanilla JS** | No build step; embed directly in binary |
| Asset embedding | **include_bytes!** / **include_str!** | Single binary distribution, no external files |
| Sprites | **PNG sprite sheets** | Standard pixel art format, embedded in binary |

### Rust Changes

```
src/
  main.rs          ← add `view` subcommand, start server
  server.rs        ← NEW: axum routes, SSE endpoint, static file serving
  session.rs       ← unchanged
  app.rs           ← extract shared refresh logic for both modes
  ui.rs            ← unchanged (TUI mode)
  web/
    index.html     ← single-page Canvas 2D app (embedded)
    style.css      ← minimal layout styles (embedded)
    app.js         ← room layout, character rendering, animations (embedded)
    sprites.png    ← sprite sheet with all character states (embedded)
```

### SSE Payload

The SSE endpoint sends the full session list as JSON every 2 seconds:

```json
{
  "sessions": [
    {
      "session_id": "abc123",
      "tmux_session": "refactor-auth",
      "project_name": "recon",
      "branch": "feat/auth",
      "cwd": "/Users/gavra/repos/recon",
      "room": "recon",
      "status": "Working",
      "model_display": "Opus 4.6",
      "total_input_tokens": 45000,
      "total_output_tokens": 12000,
      "context_window": 1000000,
      "token_ratio": 0.057,
      "last_activity": "< 1m",
      "started_at": 1710000000
    }
  ]
}
```

## Interaction

### Already Implemented

- **Room zoom**: `1-4` zooms into a room, `Esc` zooms back out
- **Page navigation**: `h/l` (or arrow keys) to page through rooms (4 per page)
- **Mode toggle**: `v` switches between table and view mode
- **Refresh**: `r` forces a refresh

### Phase 3: Agent Selection and Switching

When zoomed into a room, you should be able to select individual agents and interact with them — matching the table mode's capabilities.

**Agent cursor** (zoomed-in room only):
- `j/k` or arrow keys to move selection between agents in the room
- Visual highlight on the selected agent (border glow or underline)
- Selected agent shows extra detail: model, full context bar, last activity

**Switch to agent**:
- `Enter` on a selected agent runs `tmux switch-client -t {session}` and exits recon (same as table mode)
- This switches the current tmux client — the user returns to recon by switching back

**Kill agent**:
- `x` on a selected agent kills the tmux session (same as table mode, with confirmation)

**Create session**:
- `n` opens the new-session form (same as table mode)

## Implementation Phases

### Phase 1: Static Dashboard (done)
- `recon view` subcommand (TUI, not browser — pivoted from original axum/Canvas plan)
- Room layout: 2x2 grid with pagination (`h/l`), zoom (`1-4`), `Esc` to zoom out
- Half-block pixel sprites per state (New=egg, Working=green, Idle=sleeping, Input=angry)
- Session name, git branch, status label, context bar per agent
- Room grouping by full CWD path (shown with `~` prefix)
- Auto-refresh every 2 seconds, `v` toggles to table mode

### Phase 2: Animations and Polish (done)
- Tick-based animations: Working and Input sprites animate (3 frames each), Idle and New are static
- Phase-offset per session ID so agents in the same room don't animate in sync
- Input rooms: border pulses yellow/white, agent label pulses
- Context bar with color coding (green/yellow/red)
- Fatigue overlay concept (high context → visual change) — designed but not yet visible at low usage

### Phase 3: Interaction (next)
- Agent cursor: `j/k` to select agents within a zoomed room
- `Enter` to switch to selected agent's tmux session
- `x` to kill selected agent
- `n` to create a new session from view mode

## Design Principles

1. **Glanceable**: You should know the health of all agents in <1 second from across a room
2. **Emotionally engaging**: The Tamagotchi metaphor makes monitoring feel like caretaking, not chore work
3. **Zero config**: Works out of the box with sensible defaults (rooms from CWD, characters from session hash)
4. **Single binary**: Everything embeds in the recon binary — no npm, no separate asset folders
5. **Additive**: This is a new mode alongside the existing TUI, not a replacement
