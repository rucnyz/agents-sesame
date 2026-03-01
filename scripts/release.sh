#!/bin/bash
set -euo pipefail

# Usage: ./scripts/release.sh <patch|minor|major>

TYPE="${1:-}"
if [[ "$TYPE" != "patch" && "$TYPE" != "minor" && "$TYPE" != "major" ]]; then
    echo "Usage: ./scripts/release.sh <patch|minor|major>"
    exit 1
fi

# Read current version from Cargo.toml
CURRENT=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$TYPE" in
    major) NEW="$((MAJOR + 1)).0.0" ;;
    minor) NEW="$MAJOR.$((MINOR + 1)).0" ;;
    patch) NEW="$MAJOR.$MINOR.$((PATCH + 1))" ;;
esac

echo "Bumping $CURRENT -> $NEW"

# Update Cargo.toml version
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW\"/" Cargo.toml

# Verify it builds
echo "Building..."
cargo build --release

# Commit, tag, push
git add Cargo.toml Cargo.lock
git commit -m "release v$NEW"
git tag "v$NEW"
git push
git push --tags

# Publish to crates.io
echo "Publishing to crates.io..."
cargo publish

echo ""
echo "Released v$NEW"
echo "CD workflow will build and upload binaries to GitHub Releases."
echo "Published to https://crates.io/crates/agents-sesame"
