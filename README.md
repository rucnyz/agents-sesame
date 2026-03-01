# agents-sesame (ase)

[![Crates.io](https://img.shields.io/crates/v/agents-sesame)](https://crates.io/crates/agents-sesame)
[![CI](https://github.com/rucnyz/agents-sesame/actions/workflows/ci.yml/badge.svg)](https://github.com/rucnyz/agents-sesame/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Fast fuzzy finder TUI for coding agent session history. Search and resume sessions across 10 coding agents.

## Supported Agents

Claude Code, Codex CLI, GitHub Copilot CLI, Copilot VSCode, Crush, Gemini CLI, Kimi CLI, OpenCode, Qwen Code, Vibe

## Install

### Binary (recommended)

```sh
curl -fsSL https://rucnyz.github.io/agents-sesame/install.sh | bash
```

Installs `ase` (+ `agents-sesame` symlink) to `~/.local/bin/`. Supports Linux (x86_64/aarch64) and macOS (Intel/Apple Silicon).

### crates.io

```sh
cargo install agents-sesame
```

### From source (latest dev)

```sh
cargo install --git https://github.com/rucnyz/agents-sesame
```

### Build from scratch

```sh
git clone https://github.com/rucnyz/agents-sesame.git
cd agents-sesame
cargo build --release
# binary at target/release/ase
```

## Usage

```sh
ase                          # Open TUI
ase --list                   # List sessions to stdout
ase --list 'niri'            # Search and list
ase --agent claude --list    # Filter by agent
ase --rebuild --list         # Force rebuild index
ase --stats                  # Show index stats

# Scriptable CLI (for fzf, television, pipes)
ase --list --format=tsv      # Tab-delimited output
ase --list --format=json     # JSON lines (one object per line)
ase --preview <session-id>   # Print session content to stdout
ase --resume <session-id>    # Resume session directly by ID

# Management
ase init                     # Set up shell integration (Alt+G + completions)
ase update                   # Self-update to latest release
ase uninstall                # Remove binary, config, cache, and shell integration
```

## Keybindings

| Key | Action |
|-----|--------|
| `↑/↓` or `j/k` | Navigate results |
| `Enter` | Resume selected session |
| `Tab` / `Shift+Tab` | Cycle agent filter |
| `Ctrl+S` | Toggle sort (relevance / time) |
| `Ctrl+D` | Cycle directory scope |
| `Ctrl+U` | Clear search |
| `` Ctrl+` `` | Toggle preview |
| `Ctrl+P` | Toggle preview layout |
| `c` | Copy session content |
| `Ctrl+E` | Toggle mouse capture |
| `Esc` | Quit |

## Search Syntax

| Prefix | Example | Meaning |
|--------|---------|---------|
| `agent:` | `agent:claude` | Filter by agent |
| `-agent:` | `-agent:opencode` | Exclude agent |
| `dir:` | `dir:agents-sesame` | Filter by directory |
| `date:` | `date:today`, `date:3d`, `date:1w` | Filter by time |

## Config

Optional config at `~/.config/agents-sesame/config.toml`:

```toml
[agents.claude]
dir = "~/.claude/projects"

[agents.opencode]
db = "~/.local/share/opencode/opencode.db"
```

### Theme (Material You)

Customize TUI colors with a `[theme]` section using Material You role names:

```toml
[theme]
primary = "#E87B35"              # accent: borders, title, footer keys
on_surface = "#FFFFFF"           # normal text
on_surface_variant = "#808080"   # dim text, inactive borders
surface_variant = "#28283C"      # selected row background
surface_container = "#3C3C3C"    # scrollbar track
secondary = "#64C8FF"            # secondary accent (user message prefix, project scope)
tertiary = "#64FF64"             # tertiary accent (local scope, loading, status)
primary_container = "#FFFF00"    # search highlight match
error = "#FF0000"                # error color
```

All fields are optional — unset values use built-in defaults.

## Shell Integration

Set up <kbd>Alt+G</kbd> keybinding and tab completions:

```sh
ase init               # auto-detect shell, writes to config
ase init fish          # explicit shell
```

## Integrations

### fzf

```bash
ase --list --format=tsv | fzf --delimiter='\t' --with-nth=2,3,4,5,6 \
  --preview='ase --preview {1}' \
  --bind='enter:become(ase --resume {1})'
```

### television / matugen

See the [Integrations](https://rucnyz.github.io/agents-sesame/integrations.html) page for television cable channel and matugen auto-theme setup.

## Roadmap

- Semantic search (embedding-based similarity)
- HTTP server mode (optional feature)
- MCP server mode
- AI skills / tool-use integration
- Auto-refresh (filesystem watch)
- Session export (markdown / HTML)

## License

MIT
