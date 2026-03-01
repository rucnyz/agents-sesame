#!/usr/bin/env bash
# Cross-project benchmark: fr-rs vs other session search tools
# Prerequisites: build each project and have binaries available
set -euo pipefail

SESSIONS_DIR="${SESSIONS_DIR:-$HOME/src/sessions}"
FR_RS="${BASH_SOURCE[0]%/*}/../target/release/fr-rs"
FR_PY="fr"
AGF="$SESSIONS_DIR/agf/target/release/agf"
CC_SESSIONS="$SESSIONS_DIR/cc-sessions/target/release/cc-sessions"
CCRIDER="$SESSIONS_DIR/ccrider/ccrider"
CCSEARCH="$SESSIONS_DIR/ccsearch/target/release/ccsearch"

if ! command -v hyperfine &>/dev/null; then
    echo "hyperfine not found. Install: paru -S hyperfine"
    exit 1
fi

echo "========================================="
echo "  Session Search Tools Benchmark"
echo "========================================="

# Collect available tools
TOOLS=("fr-rs")
[ -x "$FR_PY" ] || command -v "$FR_PY" &>/dev/null && TOOLS+=("fr-py")
[ -x "$AGF" ] && TOOLS+=("agf")
[ -x "$CC_SESSIONS" ] && TOOLS+=("cc-sessions")
[ -x "$CCRIDER" ] && TOOLS+=("ccrider")
[ -x "$CCSEARCH" ] && TOOLS+=("ccsearch")
echo "Available tools: ${TOOLS[*]}"
echo ""

echo "=== 1. List sessions (warm) ==="
ARGS=(-n "fr-rs" "$FR_RS --list 2>/dev/null")
[ -x "$CC_SESSIONS" ] && ARGS+=(-n "cc-sessions" "$CC_SESSIONS --list 2>/dev/null")
command -v "$FR_PY" &>/dev/null && ARGS+=(-n "fr (Python)" "$FR_PY --no-tui --list 2>/dev/null")
[ -x "$CCRIDER" ] && ARGS+=(-n "ccrider" "$CCRIDER list 2>/dev/null")
[ -x "$CCSEARCH" ] && ARGS+=(-n "ccsearch" "$CCSEARCH list 2>/dev/null")
hyperfine --warmup 2 --min-runs 10 "${ARGS[@]}"

echo ""
echo "=== 2. Search: 'niri' ==="
ARGS=(-n "fr-rs" "$FR_RS --list 'niri' 2>/dev/null")
command -v "$FR_PY" &>/dev/null && ARGS+=(-n "fr (Python)" "$FR_PY --no-tui 'niri' 2>/dev/null")
[ -x "$AGF" ] && ARGS+=(-n "agf" "$AGF resume 'niri' 2>/dev/null")
[ -x "$CCRIDER" ] && ARGS+=(-n "ccrider" "$CCRIDER search 'niri' 2>/dev/null")
[ -x "$CCSEARCH" ] && ARGS+=(-n "ccsearch" "$CCSEARCH search 'niri' --no-tui 2>/dev/null")
hyperfine --warmup 2 --min-runs 10 "${ARGS[@]}"

echo ""
echo "=== 3. TUI startup + quit ==="
ARGS=(-n "fr-rs" "echo -ne '\x1b' | $FR_RS 2>/dev/null || true")
command -v "$FR_PY" &>/dev/null && ARGS+=(-n "fr (Python)" "echo -ne '\x1b' | $FR_PY 2>/dev/null || true")
[ -x "$AGF" ] && ARGS+=(-n "agf" "echo -ne '\x1b' | $AGF 2>/dev/null || true")
hyperfine --warmup 1 --min-runs 5 "${ARGS[@]}"

echo ""
echo "=== 4. Binary sizes ==="
[ -x "$FR_RS" ] && ls -lh "$FR_RS" | awk '{print "fr-rs:", $5}'
[ -x "$AGF" ] && ls -lh "$AGF" | awk '{print "agf:", $5}'
[ -x "$CC_SESSIONS" ] && ls -lh "$CC_SESSIONS" | awk '{print "cc-sessions:", $5}'
[ -x "$CCRIDER" ] && ls -lh "$CCRIDER" | awk '{print "ccrider:", $5}'
[ -x "$CCSEARCH" ] && ls -lh "$CCSEARCH" | awk '{print "ccsearch:", $5}'

echo ""
echo "=== 5. Supported agents ==="
echo "fr-rs: 10 (claude, codex, copilot-cli, copilot-vscode, crush, gemini, kimi, opencode, qwen, vibe)"
echo "agf: 7 (claude, codex, opencode, pi, kiro, cursor, gemini)"
echo "cc-sessions: 1 (claude)"
echo "ccrider: 2 (claude, codex)"
echo "ccsearch: 1 (claude)"
