# rust-resume (fr-rs)

Fast fuzzy finder TUI for coding agent session history. Search and resume sessions across 10 coding agents.

## Supported Agents

Claude Code, Codex CLI, GitHub Copilot CLI, Copilot VSCode, Crush, Gemini CLI, Kimi CLI, OpenCode, Qwen Code, Vibe

## Install

### Binary (recommended)

```sh
curl -fsSL https://rucnyz.github.io/rust-resume/install.sh | bash
```

Installs to `~/.local/bin/fr-rs`. Supports Linux (x86_64/aarch64) and macOS (Intel/Apple Silicon).

### From source

```sh
# requires Rust toolchain (https://rustup.rs)
cargo install --git https://github.com/rucnyz/rust-resume
```

### Build from scratch

```sh
git clone https://github.com/rucnyz/rust-resume.git
cd rust-resume
cargo build --release
# binary at target/release/fr-rs
```

## Usage

```sh
fr-rs                          # Open TUI
fr-rs --list                   # List sessions to stdout
fr-rs --list 'niri'            # Search and list
fr-rs --agent claude --list    # Filter by agent
fr-rs --rebuild --list         # Force rebuild index
fr-rs --stats                  # Show index stats

# Scriptable CLI (for fzf, television, pipes)
fr-rs --list --format=tsv      # Tab-delimited output
fr-rs --list --format=json     # JSON lines (one object per line)
fr-rs --preview <session-id>   # Print session content to stdout
fr-rs --resume <session-id>    # Resume session directly by ID

# Management
fr-rs init                     # Set up shell integration (Alt+G + completions)
fr-rs update                   # Self-update to latest release
fr-rs uninstall                # Remove binary, config, cache, and shell integration
```

## Keybindings

| Key | Action |
|-----|--------|
| `↑/↓` or `j/k` | Navigate results |
| `Enter` | Resume selected session |
| `Tab` / `Shift+Tab` | Cycle agent filter |
| `Ctrl+S` | Toggle sort (relevance / time) |
| `Ctrl+U/D` | Scroll preview |
| `` Ctrl+` `` | Toggle preview |
| `Ctrl+P` | Toggle preview layout |
| `c` | Copy resume command |
| `Ctrl+E` | Toggle mouse capture |
| `Esc` | Quit |

## Search Syntax

| Prefix | Example | Meaning |
|--------|---------|---------|
| `agent:` | `agent:claude` | Filter by agent |
| `-agent:` | `-agent:opencode` | Exclude agent |
| `dir:` | `dir:rust-resume` | Filter by directory |
| `date:` | `date:today`, `date:3d`, `date:1w` | Filter by time |

## Config

Optional config at `~/.config/rust-resume/config.toml`:

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
fr-rs init               # auto-detect shell, writes to config
fr-rs init fish          # explicit shell
```

## Integrations

### fzf

```bash
fr-rs --list --format=tsv | fzf --delimiter='\t' --with-nth=2,3,4,5,6 \
  --preview='fr-rs --preview {1}' \
  --bind='enter:become(fr-rs --resume {1})'
```

### television

Copy the cable channel config to your television config:

```sh
cp docs/television-channel.toml ~/.config/television/cable/fr-rs.toml
```

Then run:

```sh
tv fr-rs
```

### matugen (auto-theme from wallpaper)

1. Copy the template:

```sh
cp docs/matugen-template.toml ~/.config/matugen/templates/fr-rs.toml
```

2. Add to your matugen config (`~/.config/matugen/config.toml`):

```toml
[templates.fr-rs]
input_path = "~/.config/matugen/templates/fr-rs.toml"
output_path = "~/.config/rust-resume/config.toml"
```

3. Run matugen — fr-rs will pick up the generated theme on next launch:

```sh
matugen image /path/to/wallpaper.jpg
```

## License

MIT
