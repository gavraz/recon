#!/usr/bin/env bash
# Deterministic, claude-free discovery tests.
#
# Unlike e2e.sh (which drives real `claude` and asserts timing-dependent
# states), these tests fabricate the four data sources recon joins
# (tmux panes, ~/.claude/sessions/{PID}.json, ~/.claude/projects/*.jsonl,
# pane content) using a `node`-backed stub pane and a scratch $HOME. That
# makes them fast and deterministic — suitable for CI without claude.
#
# Requires: tmux, node, jq, lsof.
set -euo pipefail

RECON="$(cd "$(dirname "$0")/.." && pwd)/target/debug/recon"
PASS=0
FAIL=0

RID=$(head -c 100 /dev/urandom | LC_ALL=C tr -dc 'a-z0-9' | head -c 4)
SANDBOX="/tmp/recon-disc-${RID}"
HOME_DIR="$SANDBOX/home"
SESSIONS_DIR="$HOME_DIR/.claude/sessions"
PROJ_DIR="$HOME_DIR/.claude/projects/-tmp-disc-${RID}"
PANE_CWD="/tmp/disc-${RID}-cwd"

report() {
    local result="$1" label="$2"
    if [[ "$result" == "pass" ]]; then
        echo "[PASS] $label"; (( PASS++ )) || true
    else
        echo "[FAIL] $label"; (( FAIL++ )) || true
    fi
}

cleanup() {
    tmux list-sessions -F '#{session_name}' 2>/dev/null \
        | grep "^disc-${RID}-" \
        | while read -r s; do tmux kill-session -t "$s" 2>/dev/null || true; done
    rm -rf "$SANDBOX" "$PANE_CWD"
}
trap cleanup EXIT

# --- Preflight ---
for bin in tmux node jq lsof; do
    if ! command -v "$bin" &>/dev/null; then
        echo "SKIP: '$bin' is required but not found"
        exit 0
    fi
done
if [[ ! -x "$RECON" ]]; then
    echo "FATAL: recon binary not found at $RECON (run 'cargo build' first)"
    exit 1
fi

mkdir -p "$SESSIONS_DIR" "$PROJ_DIR" "$PANE_CWD"

# recon_json <session_name> <field>: read one field for a tmux session.
recon_json() {
    HOME="$HOME_DIR" "$RECON" json 2>/dev/null \
        | jq -r --arg n "$1" ".sessions[] | select(.tmux_session == \$n) | .$2"
}

# spawn_stub <session> <pid_file_sessionid> <jsonl_basename> <input_tokens>:
# start a node-backed pane that keeps the JSONL open (as real claude does),
# write its {pid}.json advertising <pid_file_sessionid>, and create a JSONL
# named <jsonl_basename> carrying <input_tokens>. When the two session-ids
# differ this mimics a resumed session (claude appends to the original JSONL
# while sessions/{pid}.json advertises a fresh id).
spawn_stub() {
    local sess="$1" adv_sid="$2" jsonl_base="$3" tokens="$4"
    local jsonl_path="$PROJ_DIR/${jsonl_base}.jsonl"
    local now_ms=$(( $(date +%s) * 1000 ))
    local iso; iso=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)

    if (( tokens > 0 )); then
        printf '%s\n' "{\"type\":\"assistant\",\"timestamp\":\"$iso\",\"cwd\":\"$PANE_CWD\",\"message\":{\"model\":\"claude-opus-4-8\",\"usage\":{\"input_tokens\":$tokens,\"output_tokens\":50,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}" > "$jsonl_path"
    else
        : > "$jsonl_path"
    fi

    # node keeps an fd open on the JSONL, exactly like a live claude process
    JF="$jsonl_path" tmux new-session -d -s "$sess" -c "$PANE_CWD" \
        "node -e 'require(\"fs\").openSync(process.env.JF,\"r\");setInterval(()=>{},1e9)'"
    sleep 0.3

    local ppid; ppid=$(tmux list-panes -t "$sess" -F '#{pane_pid}')
    printf '{"pid":%s,"sessionId":"%s","startedAt":%s}\n' "$ppid" "$adv_sid" "$now_ms" \
        > "$SESSIONS_DIR/${ppid}.json"
}

echo "Discovery test run ID: $RID (HOME=$HOME_DIR)"

# --- Test: resumed session must not be misattributed as New (issue #22) ---
# A resumed session advertises a fresh sessionId in sessions/{pid}.json while
# claude keeps appending to the original JSONL. recon must relink the two and
# show the real token count, not fall back to the "New" 0-token placeholder.
if true; then
    ADV_SID="99999999-dead-beef-cafe-999999999999"   # id in sessions/{pid}.json
    ORIG_SID="11111111-2222-3333-4444-555555555555"  # id of the on-disk JSONL
    spawn_stub "disc-${RID}-resumed" "$ADV_SID" "$ORIG_SID" 8000

    STATUS=$(recon_json "disc-${RID}-resumed" "status")
    TOKENS=$(recon_json "disc-${RID}-resumed" "total_input_tokens")

    if [[ "$STATUS" != "New" ]] && [[ "$TOKENS" =~ ^[0-9]+$ ]] && (( TOKENS == 8000 )); then
        report pass "Resumed session relinked: status=$STATUS tokens=$TOKENS"
    else
        echo "  Expected non-New status with 8000 tokens; got status='$STATUS' tokens='$TOKENS'"
        HOME="$HOME_DIR" "$RECON" json 2>/dev/null \
            | jq -r --arg n "disc-${RID}-resumed" '.sessions[] | select(.tmux_session == $n)' \
            | sed 's/^/    /'
        report fail "Resumed session misattributed as New (issue #22)"
    fi
fi

# --- Test: a genuinely brand-new session (empty JSONL) still shows New ---
# Guards against the fix over-correcting and hiding real New sessions.
if true; then
    NEW_SID="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    spawn_stub "disc-${RID}-fresh" "$NEW_SID" "$NEW_SID" 0

    STATUS=$(recon_json "disc-${RID}-fresh" "status")
    if [[ "$STATUS" == "New" ]]; then
        report pass "Brand-new session still shows New: status=$STATUS"
    else
        report fail "Brand-new session should be New, got status='$STATUS'"
    fi
fi

echo
echo "Discovery results: $PASS passed, $FAIL failed"
(( FAIL == 0 ))
