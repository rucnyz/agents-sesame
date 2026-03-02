#!/usr/bin/env bash
# Run high-load benchmark in Docker.
# Usage: run_docker_bench.sh [multiplier] [tool1,tool2,...]
#   multiplier: data duplication factor (default: 10)
#   tools: comma-separated list of tools to benchmark (default: all available)
#          available: ase,cass,cc-sessions,ccrider,ccsearch
#
# Examples:
#   bash bench/run_docker_bench.sh 10              # all tools, 10x
#   bash bench/run_docker_bench.sh 50 ase,ccrider  # ase vs ccrider, 50x
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
SESSIONS_DIR="${SESSIONS_DIR:-$HOME/src/sessions}"
MUL="${1:-10}"
BENCH_TOOLS="${2:-}"

TMPDIR=$(mktemp -d)
trap "echo 'Cleaning up...'; rm -rf $TMPDIR" EXIT

BINARIES_DIR="$TMPDIR/binaries"
DATA_DIR="$TMPDIR/data"
mkdir -p "$BINARIES_DIR"

echo "=== Collecting binaries ==="
# Always copy ase
cp "$PROJECT_DIR/target/release/ase" "$BINARIES_DIR/" 2>/dev/null && echo "  ase" || echo "  ase: not found (run cargo build --release)"

# Copy other tools only if needed (no filter = copy all, or tool in filter list)
should_copy() {
    [ -z "$BENCH_TOOLS" ] || echo ",$BENCH_TOOLS," | grep -q ",$1,"
}
should_copy cass && cp "$SESSIONS_DIR/cass/target/release/cass" "$BINARIES_DIR/" 2>/dev/null && echo "  cass" || true
should_copy cc-sessions && cp "$SESSIONS_DIR/cc-sessions/target/release/cc-sessions" "$BINARIES_DIR/" 2>/dev/null && echo "  cc-sessions" || true
should_copy ccrider && cp "$SESSIONS_DIR/ccrider/ccrider" "$BINARIES_DIR/" 2>/dev/null && echo "  ccrider" || true
should_copy ccsearch && cp "$SESSIONS_DIR/ccsearch/target/release/ccsearch" "$BINARIES_DIR/" 2>/dev/null && echo "  ccsearch" || true

echo ""
echo "=== Generating ${MUL}x load data ==="
bash "$SCRIPT_DIR/gen_load.sh" "$HOME/.claude/projects" "$DATA_DIR" "$MUL"

echo ""
echo "=== Building Docker image ==="
cp "$SCRIPT_DIR/Dockerfile" "$TMPDIR/"
cp "$SCRIPT_DIR/docker_bench.sh" "$TMPDIR/"
docker build -t ase-bench "$TMPDIR"

echo ""
echo "=== Running benchmark in Docker ==="
docker run --rm \
    -v "$DATA_DIR:/home/bench/.claude/projects:ro" \
    ase-bench
