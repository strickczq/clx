# clx — Claude eXecutor

Interactive profile launcher for
[Claude Code](https://docs.anthropic.com/en/docs/claude-code).\
Written in Rust.

## Features

- **Profile management** — define multiple Claude profiles with different
  models, providers, and settings
- **Interactive picker** — fuzzy-select a profile by name via
  [fzf](https://github.com/junegunn/fzf), with a live preview of the resolved
  environment (optional — without fzf, clx lists the profiles and you pass one
  directly)
- **Profile inheritance** — `extends` chain lets derived profiles inherit from a
  base
- **TOML config** — human-friendly configuration at `~/.config/clx/config.toml`
- **Direct mode** — `clx <profile>` skips the picker
- **Passthrough** — `clx <profile> -- --resume` passes args through to `claude`

## Installation

```bash
cargo install --git https://github.com/strickczq/clx
```

Or build from source:

```bash
git clone https://github.com/strickczq/clx.git
cd clx
cargo build --release
cp target/release/clx ~/.local/bin/
```

## Configuration

Create `~/.config/clx/config.toml`:

```toml
[[profiles]]
name = "default"
description = "Anthropic API"
models.default = "opus"

[[profiles]]
name = "work"
extends = "default"
description = "via custom gateway"
provider.base_url = "https://gateway.example.com"
provider.env_key = "WORK_API_TOKEN"
```

### Profile Fields

| Field                   | Description                                             |
| ----------------------- | ------------------------------------------------------- |
| `extends`               | Parent profile to inherit from                          |
| `description`           | Human-readable description (shown in picker preview)    |
| `models.default`        | Default model (`ANTHROPIC_MODEL`)                       |
| `models.small_fast`     | Small/fast model (`ANTHROPIC_SMALL_FAST_MODEL`)         |
| `models.default_haiku`  | Haiku model (`ANTHROPIC_DEFAULT_HAIKU_MODEL`)           |
| `models.default_sonnet` | Sonnet model (`ANTHROPIC_DEFAULT_SONNET_MODEL`)         |
| `models.default_opus`   | Opus model (`ANTHROPIC_DEFAULT_OPUS_MODEL`)             |
| `provider.base_url`     | API base URL (`ANTHROPIC_BASE_URL`)                     |
| `provider.env_key`      | Env var holding the auth token                          |
| `auto_compact_pct`      | Auto-compaction threshold % (1-100)                     |
| `auto_compact_window`   | Auto-compaction window size                             |
| `skip_permissions`      | Pass `--dangerously-skip-permissions` (default `false`) |

`skip_permissions` may also be set in the `[global]` table as a default for
every profile; a profile's own value (including via `extends`) takes precedence:

```toml
[global]
skip_permissions = false   # default for all profiles

[[profiles]]
name = "yolo"
skip_permissions = true    # overrides the global default
```

## Usage

```bash
# Interactive picker
clx

# Launch with a specific profile
clx work

# Pass arguments through to claude
clx work -- --resume

# No profile → interactive picker, then passthrough
clx -- --resume

# List available profiles (non-interactive)
clx --list

# Print version
clx --version
```

## How It Works

1. **Config loading** — reads `~/.config/clx/config.toml`
2. **Profile selection** — interactive fzf picker or CLI argument
3. **Inheritance resolution** — walks the `extends` chain, merges
   models/provider/auto-compaction
4. **Launch** — builds environment and `execve`s into `claude`

The launcher **replaces** its own process with `claude`, so there is no wrapper
process left running.

## License

MIT
