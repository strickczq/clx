# clx — Claude eXecutor

Interactive profile launcher for
[Claude Code](https://docs.anthropic.com/en/docs/claude-code).\
Written in Rust.

## Features

- **Profile management** — a profile is just env vars + args, so supporting a
  new Claude Code variable or flag takes a config line, not a release
- **Reference values** — `$VAR` keeps secrets out of the config file (and out
  of the picker preview)
- **Interactive picker** — fuzzy-select a profile by name via
  [fzf](https://github.com/junegunn/fzf), with a live preview of the resolved
  environment and launch command (optional — without fzf, clx lists the
  profiles and you pass one directly)
- **Profile inheritance** — `extends` chain; env merges per key, args per flag
- **TOML config** — human-friendly configuration at `~/.config/clx/config.toml`
- **Direct mode** — `clx <profile>` skips the picker
- **Passthrough** — `clx <profile> -- --model sonnet` merges args over the
  profile's own; the command line wins on the same flag

## Installation

With [mise](https://mise.jdx.dev) (prebuilt binary from the latest release):

```bash
mise use -g github:strickczq/clx
# or pin a version
mise use -g github:strickczq/clx@v0.5.0
```

With cargo:

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
[global]
# Merged under every profile; a profile's own value wins per key.
env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = "1"

[[profiles]]
name = "work"
description = "via custom gateway"
env.ANTHROPIC_BASE_URL = "https://gateway.example.com"
env.ANTHROPIC_AUTH_TOKEN = "$WORK_API_TOKEN"
env.ANTHROPIC_MODEL = "opus"
args = ["--dangerously-skip-permissions"]

[[profiles]]
name = "work-sonnet"
extends = "work"
description = "same gateway, cheaper model"
env.ANTHROPIC_MODEL = "sonnet"
```

### Profile fields

| Field         | Description                                             |
| ------------- | ------------------------------------------------------- |
| `name`        | Profile name shown in the picker                        |
| `extends`     | Parent profile to inherit from                          |
| `description` | Human-readable description (shown in picker preview)    |
| `hidden`      | Keep out of the picker and `--list` (extend-only bases) |
| `env`         | Environment variables, keyed by the exact variable name |
| `args`        | Arguments passed to `claude`                            |

`[global]` takes the same `env` and `args` and is the implicit root of every
profile: it is merged first, then the profile's own values (including via
`extends`) take precedence.

A `hidden` profile exists only to be extended by others: it is omitted from the
picker, the fzf-missing fallback list, and `--list`, but `clx <name>` still
launches it directly and `extends` resolves it as usual.

### env

- Keys are literal variable names — clx assigns them without interpreting them.
  (Unknown *fields* elsewhere in a profile are rejected at parse time, with a
  `did you mean` hint on a close typo; env variable names themselves are
  freeform.)
- Values may be strings, integers, floats, or booleans; they are stringified, so
  `env.CLAUDE_CODE_AUTO_COMPACT_WINDOW = 786432` works. Arrays, tables, and
  datetimes are rejected.
- Values support `$VAR` and `${VAR}` references, resolved from clx's own
  environment at launch. A missing variable is an error; `$$` is a literal `$`;
  malformed references are rejected when the config is parsed, with the line
  number.
- Keep secrets in a reference: the picker preview shows values exactly as
  written, so a literal token would be displayed.

### args

`args` is a list of tokens handed to `claude` verbatim. clx knows a table of
`claude --help` flags (in `src/claude_flags.rs`) and uses it to merge across
`[global]`,
the `extends` chain, and the command line:

- A flag clx knows **takes its key over**: every inherited item with that flag
  is dropped, and the new one lands at the tail — a `--model opus` from a base
  profile is overridden by the child's `--model sonnet`, or by
  `clx work -- --model sonnet` on the command line. Repeating the flag within
  one list keeps all its occurrences.
- Anything clx doesn't know — positionals, flags newer than the table — is
  appended verbatim.
- List-valued flags (`--add-dir`, `--mcp-config`, `--plugin-dir`, …) accumulate
  instead of taking over.
- Items from a later generation land before an inherited `--`, so the command
  line's flags stay flags.
- There is no removal: a profile cannot unset a flag it inherits, so keep flags
  a derived profile might disagree with out of `[global]`.
- Nothing is ever rewritten; clx moves whole tokens, never edits one.

Write value-taking flags in the self-contained `--flag=value` form. With the
spaced form, whether a following token reads as that flag's value is up to
claude's own parser — the same as if you had typed the whole line yourself. In
particular, a profile whose `args` end in a list-valued flag will swallow
positional arguments that come after it, prompt included.

The table only affects merging. A flag missing from it is still passed through
untouched, so an older clx degrades to "appends instead of merges", never to an
error.

## Usage

```bash
# Interactive picker
clx

# Launch with a specific profile
clx work

# Pass arguments through to claude; they override the profile's own flags
clx work -- --model sonnet

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
3. **Inheritance resolution** — merges `[global]`, then walks the `extends`
   chain: env per key, args per flag
4. **Launch** — expands `$VAR` in the env values, removes every inherited
   `ANTHROPIC_*` / `CLAUDE_CODE_*` variable, and `execve`s into `claude`

The launcher **replaces** its own process with `claude`, so there is no wrapper
process left running.

clx keeps the whole `ANTHROPIC_*` / `CLAUDE_CODE_*` namespace out of claude's
inherited environment, so how claude authenticates, routes, and configures
itself comes from the profile and nowhere else. Every other variable — `TERM`,
locale, display — passes through untouched unless the profile declares it, in
which case the profile's value wins.

## Changes in 0.5

0.5 replaces the typed profile fields of earlier versions (`models.*`,
`provider.*`, `auto_compact_pct`, `effort_level`, `skip_permissions`, …) with
plain `env` and `args`; configs using the old fields are rejected at parse time.
Behavior that used to be implicit is now written out:

- a custom gateway no longer gets `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` /
  `CLAUDE_CODE_ATTRIBUTION_HEADER` set automatically — put them in `env` where
  you want them
- the removed-environment set is now the whole `ANTHROPIC_*` / `CLAUDE_CODE_*`
  namespace instead of a fixed list, so nothing under those prefixes reaches
  claude from the shell anymore — declare it in a profile if you want it
- literal values show up in the picker preview; prefer `$VAR` for secrets
- flag removal has no equivalent: 0.4's per-profile opt-out (`skip_permissions =
  false` against a global `true`) cannot be expressed — keep such flags out of
  `[global]` and set them per profile instead

## License

MIT
