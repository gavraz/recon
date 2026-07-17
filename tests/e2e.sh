#!/usr/bin/env bash
set -euo pipefail

RECON="$(cd "$(dirname "$0")/.." && pwd)/target/debug/recon"
PASS=0
FAIL=0

# Random 4-char ID to avoid collisions with real sessions
RID=$(head -c 100 /dev/urandom | LC_ALL=C tr -dc 'a-z0-9' | head -c 4)
S_NEW="e2e-${RID}-new"
S_INPUT="e2e-${RID}-input"
S_TWIN="e2e-${RID}-twin"
S_RESUME_ORIG="e2e-${RID}-res-orig"
S_RESUME_NEW="e2e-${RID}-res-new"
S_RESET="e2e-${RID}-reset"
S_WORKING="e2e-${RID}-working"
S_MULTI="e2e-${RID}-multi"
S_DUAL="e2e-${RID}-dual"
TMPDIR_NEW="/tmp/recon-e2e-${RID}"
TMPDIR_INPUT="/tmp/recon-e2e-${RID}-input"
TMPDIR_RESUME="/tmp/recon-e2e-${RID}-resume"
TMPDIR_RESET="/tmp/recon-e2e-${RID}-reset"
TMPDIR_WORKING="/tmp/recon-e2e-${RID}-working"
TMPDIR_MULTI="/tmp/recon-e2e-${RID}-multi"
TMPDIR_DUAL_A="/tmp/recon-e2e-${RID}-dual-a"
TMPDIR_DUAL_B="/tmp/recon-e2e-${RID}-dual-b"
TMPFILE="/tmp/recon-e2e-${RID}-testfile.txt"
FIXTURES="$(cd "$(dirname "$0")" && pwd)/fixtures"

CLAUDE_MODEL="${CLAUDE_MODEL:-haiku}"
CLAUDE_EFFORT="${CLAUDE_EFFORT:-low}"
CLAUDE_FLAGS="--model $CLAUDE_MODEL --effort $CLAUDE_EFFORT"

# --- Test selection ---
ALL_TESTS=(new_state working_state idle_state token_stability sort_order input_state resume_tokens resume_idempotency reset_activity working_sonnet multi_pane_status pane_target_json dual_pane_discovery tags_launch tags_filter tags_multi_filter)
RUN_TESTS=("$@")

should_run() {
    local name="$1"
    [[ ${#RUN_TESTS[@]} -eq 0 ]] && return 0
    for t in "${RUN_TESTS[@]}"; do
        [[ "$t" == "$name" ]] && return 0
    done
    return 1
}

# Count tests to run
TOTAL=0
for t in "${ALL_TESTS[@]}"; do
    if should_run "$t"; then
        (( TOTAL++ )) || true
    fi
done

if [[ ${#RUN_TESTS[@]} -gt 0 ]]; then
    echo "Test run ID: $RID (model=$CLAUDE_MODEL, effort=$CLAUDE_EFFORT) — running: ${RUN_TESTS[*]}"
else
    echo "Test run ID: $RID (model=$CLAUDE_MODEL, effort=$CLAUDE_EFFORT)"
fi

# --- Cleanup ---
cleanup() {
    # Kill all tmux sessions with our random prefix
    tmux list-sessions -F '#{session_name}' 2>/dev/null \
        | grep "^e2e-${RID}-" \
        | while read -r s; do tmux kill-session -t "$s" 2>/dev/null || true; done
    rm -rf "$TMPDIR_NEW" "$TMPDIR_INPUT" "$TMPDIR_RESUME" "$TMPDIR_RESET" "$TMPDIR_WORKING" "$TMPDIR_MULTI" "$TMPDIR_DUAL_A" "$TMPDIR_DUAL_B" "$TMPFILE" "/tmp/recon-e2e-${RID}-tag"
}
trap cleanup EXIT

# --- Preflight ---
if ! command -v jq &>/dev/null; then
    echo "FATAL: jq is required but not found"
    exit 1
fi

if ! command -v claude &>/dev/null; then
    echo "FATAL: claude CLI is required but not found"
    exit 1
fi

if [[ ! -x "$RECON" ]]; then
    echo "Building recon..."
    (cd "$(dirname "$0")/.." && cargo build --quiet)
fi

# Make sure tmux server is running
tmux start-server 2>/dev/null || true

# --- Helpers ---

create_session() {
    local name="$1" cwd="$2"
    mkdir -p "$cwd"
    # Pre-authorize `sleep` only, so the working_state test's deterministic
    # `sleep` runs instead of parking at a permission prompt under manual mode.
    # Every other tool (e.g. Write in input_state) still prompts as normal.
    tmux new-session -d -s "$name" -c "$cwd" "$(which claude) $CLAUDE_FLAGS --allowedTools 'Bash(sleep *)'"
}

# Submit a prompt to a Claude Code pane.
#
# The prompt text and the Enter keystroke are sent as two separate send-keys
# calls with a short settle in between. Newer Claude TUIs treat a fast burst of
# input as a bracketed paste and swallow an Enter that arrives in the same
# instant, leaving the prompt sitting unsubmitted in the input box (which then
# reads as New/Idle forever). Sending Enter on its own after the paste settles
# submits reliably. $name may be a session name or a full pane target (e.g.
# "sess:0.0").
send_to_session() {
    local name="$1" text="$2"
    tmux send-keys -t "$name" "$text"
    sleep 0.7
    tmux send-keys -t "$name" Enter
}

get_state() {
    local name="$1"
    "$RECON" json 2>/dev/null | jq -r \
        --arg name "$name" \
        '.sessions[] | select(.tmux_session == $name) | .status' \
    || echo ""
}

get_field() {
    local name="$1" field="$2"
    "$RECON" json 2>/dev/null | jq -r \
        --arg name "$name" \
        --arg field "$field" \
        '.sessions[] | select(.tmux_session == $name) | .[$field]' \
    || echo ""
}

wait_for_state() {
    local name="$1" expected="$2" timeout="$3"
    local elapsed=0 state=""

    while (( elapsed < timeout )); do
        state="$(get_state "$name")"
        if [[ "$state" == "$expected" ]]; then
            return 0
        fi
        sleep 1
        (( elapsed++ )) || true
    done

    # Timeout — dump debug info
    echo "  TIMEOUT after ${timeout}s waiting for state '$expected' on session '$name'"
    echo "  Last seen state: '${state:-<not found>}'"
    echo "  Pane content:"
    tmux capture-pane -t "$name" -p -S -10 2>/dev/null | sed 's/^/    /' || echo "    <capture failed>"
    return 1
}

report() {
    local result="$1" label="$2"
    if [[ "$result" == "pass" ]]; then
        echo "[PASS] $label"
        (( PASS++ )) || true
    else
        echo "[FAIL] $label"
        (( FAIL++ )) || true
    fi
}

# --- Test 1: New state ---
if should_run "new_state"; then
    create_session "$S_NEW" "$TMPDIR_NEW"

    if wait_for_state "$S_NEW" "New" 15; then
        report pass "New state detected for $S_NEW"
    else
        report fail "New state detected for $S_NEW"
    fi
fi

# --- Test 2: Working state ---
if should_run "working_state"; then
    # Ensure the session exists (test may run standalone)
    if ! tmux has-session -t "$S_NEW" 2>/dev/null; then
        create_session "$S_NEW" "$TMPDIR_NEW"
        wait_for_state "$S_NEW" "New" 15 >/dev/null 2>&1 || true
    fi

    # Wait for the TUI to be fully ready for input
    sleep 3
    # A `sleep` gives a deterministic ~10s Working window, independent of model
    # generation speed (a pure-generation prompt can finish between poll cycles
    # and be missed). `sleep` is pre-authorized in create_session so it runs
    # instead of parking at a permission prompt, and the session then settles to
    # Idle for the idle_state test (which reuses this session).
    send_to_session "$S_NEW" "Run this exact bash command and nothing else: sleep 10"

    if wait_for_state "$S_NEW" "Working" 15; then
        report pass "Working state detected for $S_NEW"
    else
        report fail "Working state detected for $S_NEW"
    fi
fi

# --- Test 3: Idle state ---
if should_run "idle_state"; then
    # Ensure the session exists and has been given a prompt
    if ! tmux has-session -t "$S_NEW" 2>/dev/null; then
        create_session "$S_NEW" "$TMPDIR_NEW"
        wait_for_state "$S_NEW" "New" 15 >/dev/null 2>&1 || true
        sleep 3
        send_to_session "$S_NEW" "write a 500 word essay about the history of unix"
    fi

    if wait_for_state "$S_NEW" "Idle" 60; then
        report pass "Idle state detected for $S_NEW"
    else
        report fail "Idle state detected for $S_NEW"
    fi
fi

# --- Test 4: Token stability (same CWD, two sessions) ---
if should_run "token_stability"; then
    # Ensure S_NEW exists and is idle
    if ! tmux has-session -t "$S_NEW" 2>/dev/null; then
        create_session "$S_NEW" "$TMPDIR_NEW"
        wait_for_state "$S_NEW" "New" 15 >/dev/null 2>&1 || true
        sleep 3
        send_to_session "$S_NEW" "write a 500 word essay about the history of unix"
        wait_for_state "$S_NEW" "Idle" 60 >/dev/null 2>&1 || true
    fi

    create_session "$S_TWIN" "$TMPDIR_NEW"
    wait_for_state "$S_TWIN" "New" 15 >/dev/null 2>&1 || true

    sleep 3
    send_to_session "$S_TWIN" "say exactly: hello world"
    wait_for_state "$S_TWIN" "Idle" 20 >/dev/null 2>&1 || true

    tokens_stable=true
    prev_new="" prev_twin=""
    for i in $(seq 1 6); do
        json=$("$RECON" json 2>/dev/null)
        cur_new=$(echo "$json" | jq -r --arg n "$S_NEW" '.sessions[] | select(.tmux_session == $n) | .total_input_tokens')
        cur_twin=$(echo "$json" | jq -r --arg n "$S_TWIN" '.sessions[] | select(.tmux_session == $n) | .total_input_tokens')
        if [[ -n "$prev_new" && ("$cur_new" != "$prev_new" || "$cur_twin" != "$prev_twin") ]]; then
            echo "  Token swap detected: $S_NEW went $prev_new→$cur_new, $S_TWIN went $prev_twin→$cur_twin"
            tokens_stable=false
            break
        fi
        prev_new="$cur_new"
        prev_twin="$cur_twin"
        sleep 1
    done

    if $tokens_stable && [[ -n "$prev_new" && -n "$prev_twin" && "$prev_new" != "$prev_twin" ]]; then
        report pass "Token stability: $S_NEW=$prev_new, $S_TWIN=$prev_twin (same CWD, no swap)"
    else
        if ! $tokens_stable; then
            report fail "Token stability: values swapped between sessions sharing CWD"
        else
            report fail "Token stability: could not verify (new=$prev_new twin=$prev_twin)"
        fi
    fi
fi

# --- Test 5: Sort by last activity (most recent first) ---
if should_run "sort_order"; then
    # Ensure both sessions exist and are idle
    if ! tmux has-session -t "$S_NEW" 2>/dev/null; then
        create_session "$S_NEW" "$TMPDIR_NEW"
        wait_for_state "$S_NEW" "New" 15 >/dev/null 2>&1 || true
        sleep 3
        send_to_session "$S_NEW" "say exactly: hello"
        wait_for_state "$S_NEW" "Idle" 30 >/dev/null 2>&1 || true
    fi
    if ! tmux has-session -t "$S_TWIN" 2>/dev/null; then
        create_session "$S_TWIN" "$TMPDIR_NEW"
        wait_for_state "$S_TWIN" "New" 15 >/dev/null 2>&1 || true
        sleep 3
        send_to_session "$S_TWIN" "say exactly: hello world"
        wait_for_state "$S_TWIN" "Idle" 20 >/dev/null 2>&1 || true
    fi

    # Send a prompt to S_TWIN so it has the most recent activity
    sleep 2
    send_to_session "$S_TWIN" "say exactly: most recent"
    wait_for_state "$S_TWIN" "Idle" 20 >/dev/null 2>&1 || true

    json=$("$RECON" json 2>/dev/null)
    idx_new=$(echo "$json" | jq -r --arg n "$S_NEW" '.sessions | to_entries[] | select(.value.tmux_session == $n) | .key')
    idx_twin=$(echo "$json" | jq -r --arg n "$S_TWIN" '.sessions | to_entries[] | select(.value.tmux_session == $n) | .key')

    if [[ -n "$idx_new" && -n "$idx_twin" ]] && (( idx_twin < idx_new )); then
        report pass "Sort order: $S_TWIN (idx=$idx_twin) before $S_NEW (idx=$idx_new) — most recent activity first"
    else
        report fail "Sort order: expected $S_TWIN before $S_NEW (got idx_twin=$idx_twin idx_new=$idx_new)"
    fi
fi

# --- Test 6: Input state (permission prompt) ---
if should_run "input_state"; then
    create_session "$S_INPUT" "$TMPDIR_INPUT"
    wait_for_state "$S_INPUT" "New" 15 >/dev/null 2>&1 || true

    sleep 3
    send_to_session "$S_INPUT" "please create a new file at $TMPFILE with the text hello"

    if wait_for_state "$S_INPUT" "Input" 30; then
        report pass "Input state detected for $S_INPUT"
    else
        report fail "Input state detected for $S_INPUT"
    fi
fi

# --- Test 7: Resume session shows original token count ---
if should_run "resume_tokens"; then
    CLAUDE_PATH="$(which claude)"
    mkdir -p "$TMPDIR_RESUME"
    tmux new-session -d -s "$S_RESUME_ORIG" -c "$TMPDIR_RESUME" \
        "bash -c '$CLAUDE_PATH $CLAUDE_FLAGS 2>&1; exec bash'"

    wait_for_state "$S_RESUME_ORIG" "New" 15 >/dev/null 2>&1 || true
    sleep 3

    send_to_session "$S_RESUME_ORIG" "say exactly the words: recon resume test"
    wait_for_state "$S_RESUME_ORIG" "Idle" 30 >/dev/null 2>&1 || true

    TOKENS_BEFORE=$("$RECON" json 2>/dev/null | jq -r \
        --arg n "$S_RESUME_ORIG" \
        '.sessions[] | select(.tmux_session == $n) | .total_input_tokens')

    send_to_session "$S_RESUME_ORIG" "exit"
    sleep 4

    ORIG_SESSION_ID=$(tmux capture-pane -t "$S_RESUME_ORIG" -p -S -200 2>/dev/null \
        | grep -oE 'claude --resume [a-zA-Z0-9-]+' | tail -1 | awk '{print $NF}' || true)

    if [[ -z "$ORIG_SESSION_ID" ]]; then
        echo "  Could not parse resume session-id. Pane content:"
        tmux capture-pane -t "$S_RESUME_ORIG" -p -S -10 2>/dev/null | sed 's/^/    /'
        report fail "Resume: could not parse session-id from exit message (tokens_before=$TOKENS_BEFORE)"
    else
        echo "  Original session-id: $ORIG_SESSION_ID (tokens before exit: $TOKENS_BEFORE)"

        "$RECON" resume --id "$ORIG_SESSION_ID" --name "$S_RESUME_NEW" --no-attach 2>/dev/null || true
        sleep 8

        TOKENS_RESUMED=$("$RECON" json 2>/dev/null | jq -r \
            --arg n "$S_RESUME_NEW" \
            '.sessions[] | select(.tmux_session == $n) | .total_input_tokens')

        if [[ -n "$TOKENS_RESUMED" ]] && \
           [[ "$TOKENS_RESUMED" =~ ^[0-9]+$ ]] && \
           (( TOKENS_RESUMED > 0 )); then
            report pass "Resume: $S_RESUME_NEW shows ${TOKENS_RESUMED} tokens (original had ${TOKENS_BEFORE})"
        else
            echo "  Original tokens: $TOKENS_BEFORE, resumed tokens: '$TOKENS_RESUMED'"
            "$RECON" json 2>/dev/null | jq -r --arg n "$S_RESUME_NEW" \
                '.sessions[] | select(.tmux_session == $n)' | sed 's/^/    /'
            report fail "Resume: expected non-zero tokens for resumed session"
        fi
    fi
fi

# --- Test 8: Resume idempotency (no-op if already running) ---
if should_run "resume_idempotency"; then
    # Ensure the resumed session from test 7 exists
    if ! tmux has-session -t "$S_RESUME_NEW" 2>/dev/null; then
        echo "  Skipping: resume_idempotency requires resume_tokens to have run first"
        report fail "Resume idempotency: prerequisite session $S_RESUME_NEW not found"
    else
        SESSIONS_BEFORE=$(tmux list-sessions -F '#{session_name}' 2>/dev/null | grep "^e2e-${RID}-" | wc -l | tr -d ' ')

        RESUME_OUTPUT=$("$RECON" resume --id "$ORIG_SESSION_ID" --name "$S_RESUME_NEW" --no-attach 2>&1 || true)

        SESSIONS_AFTER=$(tmux list-sessions -F '#{session_name}' 2>/dev/null | grep "^e2e-${RID}-" | wc -l | tr -d ' ')

        if (( SESSIONS_BEFORE == SESSIONS_AFTER )); then
            report pass "Resume idempotency: no new session created (before=$SESSIONS_BEFORE, after=$SESSIONS_AFTER)"
        else
            report fail "Resume idempotency: session count changed (before=$SESSIONS_BEFORE, after=$SESSIONS_AFTER)"
        fi
    fi
fi

# --- Test 9: Session survives /reset (known limitation: data may be stale) ---
if should_run "reset_activity"; then
    create_session "$S_RESET" "$TMPDIR_RESET"
    wait_for_state "$S_RESET" "New" 15 >/dev/null 2>&1 || true

    sleep 3
    send_to_session "$S_RESET" "say exactly: before reset"
    wait_for_state "$S_RESET" "Idle" 30 >/dev/null 2>&1 || true

    # Reset the session — creates new JSONL without updating {PID}.json.
    # recon will show stale data from the old JSONL (known limitation).
    # Verify the session is still discovered (not lost).
    send_to_session "$S_RESET" "/reset"
    sleep 5

    STATUS=$(get_field "$S_RESET" "status")
    if [[ -n "$STATUS" ]]; then
        report pass "Reset: session still discovered (status=$STATUS)"
    else
        report fail "Reset: session lost after /reset"
    fi
fi

# --- Test 10: Working state with Sonnet reading large files ---
if should_run "working_sonnet"; then
    mkdir -p "$TMPDIR_WORKING"
    # Copy fixture files into the session's working directory
    cp "$FIXTURES"/*.txt "$TMPDIR_WORKING/"

    tmux new-session -d -s "$S_WORKING" -c "$TMPDIR_WORKING" \
        "$(which claude) --model sonnet --effort low"
    wait_for_state "$S_WORKING" "New" 15 >/dev/null 2>&1 || true

    sleep 3
    send_to_session "$S_WORKING" "read all .txt files in this directory and write a combined summary to summary.txt"

    if wait_for_state "$S_WORKING" "Working" 30; then
        report pass "Working state detected for $S_WORKING (sonnet reading files)"
    else
        report fail "Working state detected for $S_WORKING (sonnet reading files)"
    fi
fi

# --- Test 11: Multi-pane status (Claude pane is not the active pane) ---
# The bug: capture-pane -t <session> reads the active pane, not the Claude pane.
# To expose this, Claude must be in a state that differs from what bash looks like.
# We put Claude into Working state and make the bash pane active — without the fix,
# recon reads the bash pane (Idle) instead of the Claude pane (Working).
if should_run "multi_pane_status"; then
    mkdir -p "$TMPDIR_MULTI"
    cp "$FIXTURES"/*.txt "$TMPDIR_MULTI/"
    create_session "$S_MULTI" "$TMPDIR_MULTI"
    wait_for_state "$S_MULTI" "New" 15 >/dev/null 2>&1 || true

    # Split the window — creates pane 1 running bash
    tmux split-window -t "$S_MULTI" "bash"
    # Make the bash pane (pane 1) the active pane
    tmux select-pane -t "$S_MULTI:0.1"

    # Send a long prompt to the Claude pane specifically (pane 0, not the active pane)
    sleep 3
    send_to_session "$S_MULTI:0.0" "read all .txt files in this directory and write a combined summary to summary.txt"

    if wait_for_state "$S_MULTI" "Working" 20; then
        report pass "Multi-pane status: Working detected despite bash pane being active"
    else
        report fail "Multi-pane status: expected Working but got '$(get_state "$S_MULTI")' (reading wrong pane)"
    fi
fi


# --- Test 12: JSON output includes pane_target field ---
if should_run "pane_target_json"; then
    # Ensure we have a session to inspect
    if ! tmux has-session -t "$S_MULTI" 2>/dev/null; then
        create_session "$S_MULTI" "$TMPDIR_MULTI"
        wait_for_state "$S_MULTI" "New" 15 >/dev/null 2>&1 || true
    fi

    pane_target=$("$RECON" json 2>/dev/null | jq -r \
        --arg name "$S_MULTI" \
        '.sessions[] | select(.tmux_session == $name) | .pane_target')

    if [[ -n "$pane_target" && "$pane_target" != "null" ]] && echo "$pane_target" | grep -qE '^[^:]+:[0-9]+\.[0-9]+$'; then
        report pass "pane_target JSON: field present with correct format ($pane_target)"
    else
        report fail "pane_target JSON: expected session:window.pane format but got '$pane_target'"
    fi
fi

# --- Test 13: Two Claude panes in same tmux session (fresh + resumed) ---
# The bug: the dedup loop at discover_sessions() line ~245 uses tmux_session name
# to skip already-matched live entries. When a fresh and a resumed Claude share
# the same tmux session, the resumed one has a new session_id in PID.json that
# doesn't match any JSONL filename, so it enters the dedup loop — where
# known_tmux blocks it because the fresh session already claimed the session name.
# Result: the resumed pane is invisible to recon.
if should_run "dual_pane_discovery"; then
    CLAUDE_PATH="$(which claude)"
    mkdir -p "$TMPDIR_DUAL_A" "$TMPDIR_DUAL_B"

    # Start tmux session with Claude in pane 0, wrapped in bash so shell remains after exit
    tmux new-session -d -s "$S_DUAL" -c "$TMPDIR_DUAL_A" \
        "bash -c '$CLAUDE_PATH $CLAUDE_FLAGS 2>&1; exec bash'"

    wait_for_state "$S_DUAL" "New" 15 >/dev/null 2>&1 || true
    sleep 3

    # Do some work to get a session with tokens
    send_to_session "$S_DUAL:0.0" "say exactly: dual pane test"
    wait_for_state "$S_DUAL" "Idle" 30 >/dev/null 2>&1 || true

    # Exit Claude — bash shell remains in pane 0
    tmux send-keys -t "$S_DUAL:0.0" "exit" Enter
    sleep 4

    # Get the session-id from Claude's exit message
    DUAL_SESSION_ID=$(tmux capture-pane -t "$S_DUAL:0.0" -p -S -200 2>/dev/null \
        | grep -oE 'claude --resume [a-zA-Z0-9-]+' | tail -1 | awk '{print $NF}' || true)

    if [[ -z "$DUAL_SESSION_ID" ]]; then
        echo "  Could not parse session-id for dual pane test. Pane content:"
        tmux capture-pane -t "$S_DUAL:0.0" -p -S -10 2>/dev/null | sed 's/^/    /'
        report fail "Dual-pane discovery: could not parse session-id"
    else
        echo "  Original session-id: $DUAL_SESSION_ID"

        # Start fresh Claude in pane 0 (new session, same tmux session)
        tmux send-keys -t "$S_DUAL:0.0" "$CLAUDE_PATH $CLAUDE_FLAGS" Enter
        wait_for_state "$S_DUAL" "New" 15 >/dev/null 2>&1 || true
        sleep 3
        send_to_session "$S_DUAL:0.0" "say exactly: fresh session"
        wait_for_state "$S_DUAL" "Idle" 30 >/dev/null 2>&1 || true

        # Split window and start resumed Claude in pane 1 (same tmux session, same CWD)
        tmux split-window -t "$S_DUAL" -c "$TMPDIR_DUAL_A" \
            "$CLAUDE_PATH $CLAUDE_FLAGS --resume $DUAL_SESSION_ID"

        # Poll until recon sees 2 sessions with this tmux_session name (or timeout)
        elapsed=0
        timeout=20
        count=0
        while (( elapsed < timeout )); do
            count=$("$RECON" json 2>/dev/null | jq --arg name "$S_DUAL" \
                '[.sessions[] | select(.tmux_session == $name)] | length')
            if [[ "$count" -eq 2 ]]; then
                break
            fi
            sleep 1
            (( elapsed++ )) || true
        done

        if [[ "$count" -eq 2 ]]; then
            report pass "Dual-pane discovery: both fresh and resumed sessions detected in same tmux session"
        else
            echo "  Expected 2 sessions for tmux_session=$S_DUAL but found $count"
            "$RECON" json 2>/dev/null | jq --arg name "$S_DUAL" \
                '.sessions[] | select(.tmux_session == $name)' | sed 's/^/    /'
            report fail "Dual-pane discovery: resumed session not detected (dedup on tmux_session blocks it)"
        fi
    fi
fi

# --- Test 14: Tags appear in JSON output ---
if should_run "tags_launch"; then
    S_TAG="e2e-${RID}-tag"
    TMPDIR_TAG="/tmp/recon-e2e-${RID}-tag"
    mkdir -p "$TMPDIR_TAG"
    "$RECON" launch --name "$S_TAG" --cwd "$TMPDIR_TAG" --command "$(which claude) $CLAUDE_FLAGS" --tag env:test --tag role:worker 2>/dev/null
    wait_for_state "$S_TAG" "New" 15 >/dev/null 2>&1 || true

    TAGS=$("$RECON" json 2>/dev/null | jq -r \
        --arg name "$S_TAG" \
        '.sessions[] | select(.tmux_session == $name) | .tags')

    if echo "$TAGS" | jq -e '.env == "test" and .role == "worker"' &>/dev/null; then
        report pass "Tags: launched session has correct tags ($TAGS)"
    else
        report fail "Tags: expected env:test + role:worker but got $TAGS"
    fi
fi

# --- Test 15: Tag filter returns only matching sessions ---
if should_run "tags_filter"; then
    S_TAG="e2e-${RID}-tag"  # reuse from test 14 (or create if skipped)
    TMPDIR_TAG="/tmp/recon-e2e-${RID}-tag"
    if ! tmux has-session -t "$S_TAG" 2>/dev/null; then
        mkdir -p "$TMPDIR_TAG"
        "$RECON" launch --name "$S_TAG" --cwd "$TMPDIR_TAG" --command "$(which claude) $CLAUDE_FLAGS" --tag env:test --tag role:worker 2>/dev/null
        wait_for_state "$S_TAG" "New" 15 >/dev/null 2>&1 || true
    fi

    MATCH=$("$RECON" json --tag role:worker 2>/dev/null | jq '[.sessions[]] | length')
    NO_MATCH=$("$RECON" json --tag role:nonexistent 2>/dev/null | jq '[.sessions[]] | length')

    if [[ "$MATCH" -ge 1 && "$NO_MATCH" -eq 0 ]]; then
        report pass "Tags filter: --tag role:worker matched $MATCH, --tag role:nonexistent matched 0"
    else
        report fail "Tags filter: expected match>=1 got $MATCH, expected nomatch=0 got $NO_MATCH"
    fi
fi

# --- Test 16: Multiple tag filters require all to match ---
if should_run "tags_multi_filter"; then
    S_TAG="e2e-${RID}-tag"  # reuse from test 14
    TMPDIR_TAG="/tmp/recon-e2e-${RID}-tag"
    if ! tmux has-session -t "$S_TAG" 2>/dev/null; then
        mkdir -p "$TMPDIR_TAG"
        "$RECON" launch --name "$S_TAG" --cwd "$TMPDIR_TAG" --command "$(which claude) $CLAUDE_FLAGS" --tag env:test --tag role:worker 2>/dev/null
        wait_for_state "$S_TAG" "New" 15 >/dev/null 2>&1 || true
    fi

    BOTH=$("$RECON" json --tag env:test --tag role:worker 2>/dev/null | jq '[.sessions[]] | length')
    PARTIAL=$("$RECON" json --tag env:test --tag role:manager 2>/dev/null | jq '[.sessions[]] | length')

    if [[ "$BOTH" -ge 1 && "$PARTIAL" -eq 0 ]]; then
        report pass "Tags multi-filter: both match=$BOTH, partial match=$PARTIAL (AND logic)"
    else
        report fail "Tags multi-filter: expected both>=1 got $BOTH, expected partial=0 got $PARTIAL"
    fi

    # Clean up tag test session
    tmux kill-session -t "$S_TAG" 2>/dev/null || true
fi

# --- Summary ---
echo ""
if (( FAIL == 0 )); then
    echo "All $TOTAL tests passed."
else
    echo "$PASS/$TOTAL tests passed, $FAIL failed."
    exit 1
fi
