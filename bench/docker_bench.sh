#!/usr/bin/env bash
# Benchmark inside Docker container.
# Expects Claude data mounted at ~/.claude/projects
# and binaries at ~/.local/bin/{ase,cass,cc-sessions,ccrider,ccsearch}
set -euo pipefail

SESSION_COUNT=$(find ~/.claude/projects -name "*.jsonl" 2>/dev/null | wc -l)
echo "========================================="
echo "  Docker Benchmark ($SESSION_COUNT sessions)"
echo "========================================="
echo ""

ASE=ase
CASS=cass
CC_SESSIONS=cc-sessions
CCRIDER=ccrider
CCSEARCH=ccsearch

# Check which tools are available
TOOLS=()
command -v $ASE &>/dev/null && TOOLS+=("ase")
command -v $CASS &>/dev/null && TOOLS+=("cass")
command -v $CC_SESSIONS &>/dev/null && TOOLS+=("cc-sessions")
command -v $CCRIDER &>/dev/null && TOOLS+=("ccrider")
command -v $CCSEARCH &>/dev/null && TOOLS+=("ccsearch")
echo "Available tools: ${TOOLS[*]}"
echo ""

# --- 1. Cold rebuild (FIRST, before anything warms the page cache) ---
echo "=== 1. Cold rebuild (single run, cold page cache) ==="
# Run cold rebuild BEFORE any warm-up. This is the only point in the container's
# lifetime where the OS page cache is guaranteed empty — JSONL files haven't been
# read yet. Using single `time` runs (not hyperfine) because multiple iterations
# would warm the page cache, making runs 2+ artificially fast.
echo "--- ase cold rebuild ---"
rm -rf ~/.cache/agents-sesame/tantivy_index_rs
time $ASE --rebuild --list --agent claude >/dev/null 2>&1
echo ""

if command -v $CCRIDER &>/dev/null; then
    echo "--- ccrider cold rebuild ---"
    rm -f ~/.config/ccrider/sessions.db
    time $CCRIDER sync >/dev/null 2>&1
    echo ""
fi

# --- Build/warm indexes for remaining benchmarks ---
echo "=== Building indexes ==="
echo -n "ase --rebuild: "
$ASE --rebuild --list >/dev/null 2>&1 && echo "ok" || echo "fail"

if command -v $CASS &>/dev/null; then
    echo -n "cass index: "
    $CASS index >/dev/null 2>&1 && echo "ok" || echo "fail"
fi

if command -v $CCRIDER &>/dev/null; then
    echo -n "ccrider sync: "
    $CCRIDER sync >/dev/null 2>&1 && echo "ok" || echo "fail"
fi

# ccsearch and cc-sessions build index on first run
if command -v $CCSEARCH &>/dev/null; then
    echo -n "ccsearch warmup: "
    $CCSEARCH list >/dev/null 2>&1 && echo "ok" || echo "fail"
fi
if command -v $CC_SESSIONS &>/dev/null; then
    echo -n "cc-sessions warmup: "
    $CC_SESSIONS --list >/dev/null 2>&1 && echo "ok" || echo "fail"
fi
echo ""

# --- 2. List benchmark (warm cache) ---
echo "=== 2. List sessions (warm, Claude only) ==="
ARGS=(-n "ase" "$ASE --list --agent claude 2>/dev/null")
command -v $CC_SESSIONS &>/dev/null && ARGS+=(-n "cc-sessions" "$CC_SESSIONS --list 2>/dev/null")
command -v $CCRIDER &>/dev/null && ARGS+=(-n "ccrider" "$CCRIDER list 2>/dev/null")
command -v $CCSEARCH &>/dev/null && ARGS+=(-n "ccsearch" "$CCSEARCH list 2>/dev/null")
hyperfine --warmup 2 --min-runs 10 "${ARGS[@]}"

echo ""
echo "=== 3. Search 'niri' (Claude only) ==="
ARGS=(-n "ase" "$ASE --list --agent claude 'niri' 2>/dev/null")
command -v $CASS &>/dev/null && ARGS+=(-n "cass" "$CASS search 'niri' --agent claude-code --robot 2>/dev/null")
command -v $CCRIDER &>/dev/null && ARGS+=(-n "ccrider" "$CCRIDER search 'niri' 2>/dev/null")
command -v $CCSEARCH &>/dev/null && ARGS+=(-n "ccsearch" "$CCSEARCH search 'niri' --no-tui 2>/dev/null")
hyperfine --warmup 2 --min-runs 10 "${ARGS[@]}"

echo ""
echo "=== 4. Binary sizes ==="
for tool in $ASE $CASS $CC_SESSIONS $CCRIDER $CCSEARCH; do
    p=$(which "$tool" 2>/dev/null) && ls -lh "$p" | awk -v name="$tool" '{print name":", $5}' || true
done

echo ""
echo "=== Summary ==="
echo "Sessions: $SESSION_COUNT"
echo "Tools tested: ${TOOLS[*]}"
