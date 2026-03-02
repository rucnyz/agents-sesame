#!/usr/bin/env bash
# Generate high-load Claude session data by duplicating real sessions.
# Usage: gen_load.sh <source_dir> <target_dir> <multiplier>
#   source_dir: real ~/.claude/projects directory
#   target_dir: output directory (will be created)
#   multiplier: how many copies (e.g. 10 = 10x sessions)
#
# Each copy gets a unique file name AND unique sessionId inside the JSONL,
# so tools that deduplicate by internal sessionId (e.g. ccrider) also see
# the full multiplied session count.
set -euo pipefail

SRC="${1:?Usage: gen_load.sh <source_dir> <target_dir> <multiplier>}"
DST="${2:?Usage: gen_load.sh <source_dir> <target_dir> <multiplier>}"
MUL="${3:?Usage: gen_load.sh <source_dir> <target_dir> <multiplier>}"

if [ ! -d "$SRC" ]; then
    echo "Source directory not found: $SRC"
    exit 1
fi

mkdir -p "$DST"

# Count original sessions
ORIG_COUNT=$(find "$SRC" -name "*.jsonl" | wc -l)
echo "Source: $SRC ($ORIG_COUNT JSONL files)"
echo "Target: $DST (${MUL}x = $((ORIG_COUNT * MUL)) files)"

# Copy original data as round 0
echo "Copying original data..."
cp -r "$SRC"/* "$DST/"

# Generate N-1 more copies with modified UUIDs (both filename and content)
for i in $(seq 1 $((MUL - 1))); do
    echo "Generating copy $i/$((MUL - 1))..."
    PREFIX=$(printf "%08d" "$i")
    for project_dir in "$SRC"/*/; do
        project_name=$(basename "$project_dir")
        target_project="$DST/$project_name"
        mkdir -p "$target_project"

        for jsonl in "$project_dir"*.jsonl; do
            [ -f "$jsonl" ] || continue
            base=$(basename "$jsonl" .jsonl)
            # New filename: replace first 8 chars of UUID
            new_id="${PREFIX}-${base:9}"
            # Replace sessionId in JSONL content: "sessionId":"<old>" → "sessionId":"<new>"
            sed "s/\"sessionId\":\"${base}\"/\"sessionId\":\"${new_id}\"/g" \
                "$jsonl" > "$target_project/${new_id}.jsonl"
        done
    done
done

FINAL_COUNT=$(find "$DST" -name "*.jsonl" | wc -l)
echo "Done: $FINAL_COUNT JSONL files in $DST"
du -sh "$DST"
