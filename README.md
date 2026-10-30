# clx — Claude eXecutor

Interactive profile launcher for
[Claude Code](https://docs.anthropic.com/en/docs/claude-code).\
Written in Rust.

## Features

- **Profile management** — a profile is just env vars + args, so supporting a
  new Claude Code variable or flag takes a config line, not a release
- **One model per profile** — `model` fills every model variable; a subagent
  that needs another model is an [`agents`](#agents) entry
- **Model facts** — a `models` table holds each model's context, output, and
  effort once, shared by every profile that uses it
- **Subagents** — `[[profiles.agents]]` becomes claude's `--agents`, with a
  per-agent model, prompt, and any other frontmatter field
- **Reference values** — `$VAR` keeps secrets out of the config file (and out
  of the picker preview)
- **Interactive picker** — fuzzy-select a profile by name via
  [fzf](https://github.com/junegunn/fzf), with a live preview of the resolved
  environment and launch command (optional — without fzf, clx lists the
  profiles and you pass one directly)
- **Profile inheritance** — `extends` chain; env merges per key, args per flag
- **TOML config** — `config.toml`, plus `models.toml` and `conf.d/*.toml`
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

clx reads up to three kinds of file from `~/.config/clx/`, in this order:

1. `config.toml` — required, the main file
2. `models.toml` — required, model facts only; it may be empty when every
   profile uses a Claude model
3. `conf.d/*.toml` — optional, in file-name order

They merge as they are read: `env` and `models` per key, with a later file
winning on the same key; `args` append; and a profile name may appear only once
across all files. Relative prompt paths resolve against this directory.

Create `config.toml`:

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

| Field                           | Description                                             |
| ------------------------------- | ------------------------------------------------------- |
| `name`                          | Profile name shown in the picker                        |
| `extends`                       | Parent profile to inherit from                          |
| `description`                   | Human-readable description (shown in picker preview)    |
| `hidden`                        | Keep out of the picker and `--list` (extend-only bases) |
| `model`                         | Model names, optionally with a `models` table id        |
| `append_system_prompt`          | Text appended to claude's system prompt                 |
| `env`                           | Environment variables, keyed by the exact variable name |
| `args`                          | Arguments passed to `claude`                            |

`[global]` takes the same `env` and `args` and is the implicit root of every
profile: it is merged first, then the profile's own values (including via
`extends`) take precedence.

A `hidden` profile exists only to be extended by others: it is omitted from the
picker, the fzf-missing fallback list, and `--list`, but `clx <name>` still
launches it directly and `extends` resolves it as usual.

### model

`model` is the model claude runs. One name fills every model variable:
`ANTHROPIC_MODEL`, the four `ANTHROPIC_DEFAULT_*_MODEL` aliases, and
`CLAUDE_CODE_SUBAGENT_MODEL`. So `/model` and subagents stay on the same gateway
model.

```toml
model = { name = "example/model-a@500k", id = "model-a" }
```

`name` is the model name claude receives. `id` selects the [`models`
table](#models) entry that supplies token limits and effort levels. A Claude
model already has that data inside claude, so say so explicitly:

```toml
model = { name = "opus[1m]", builtin = true }
```

`id` and `builtin = true` are exclusive and one of them is required. Without
either, clx rejects the config, so a third-party model cannot silently lose its
limits. A model with an `id` may not carry a `[1m]` suffix: claude would take
1M from the name and ignore the window the `models` entry sets. Set the window
with the entry's `context` instead.

A subagent that needs its own model is defined in [`agents`](#agents); there is
no profile-level split between a main and a sub model. `CLAUDE_CODE_SUBAGENT_MODEL`
points at the main model, so an undefined subagent gets the full gateway name
instead of an alias the gateway may not recognise.

Model names take `$VAR` references like any other env value. A key the profile
writes itself in `env` wins over the shorthand. The `extends` chain treats the
expanded variables like any other env key: a child's `model` replaces all six,
and a child without `model` inherits the parent's expansion.

### models

`models` is a top-level table of facts about each model, keyed by the model id.
A profile reaches an entry through the `id` in a `model` table. A plain string
model has no `id`, so it uses none of this table.

Model entries can live in `models.toml` to keep them apart from the profiles;
the table name stays `[models.<id>]`.

Every `id` named in a `model` must have an entry here, or clx rejects the
config. `builtin = true` skips this table: claude already carries the limits
for its own models. Third-party models have no such built-in data, so they need
an entry with their context and output limits.

```toml
[models.large-model]
effort = ["effort", "xhigh_effort", "max_effort"]
limit = { context = 1000000, output = 128000 }

[models.model-a]
effort = ["effort", "xhigh_effort", "max_effort"]
limit = { context = 922000, output = 128000 }

[[profiles]]
name = "work"
model = { name = "example/large-model", id = "large-model" }
```

The entry supplies `CLAUDE_CODE_MAX_CONTEXT_TOKENS`,
`CLAUDE_CODE_MAX_OUTPUT_TOKENS`, and a `CLAUDE_CODE_MODEL_CAPABILITIES` segment
made of the model's name followed by its `effort` list. A model with no `effort`
skips the capability segment; a `builtin` model gets none of these variables. A
key the profile writes in `env` still wins.

The `limit` fields and their `context` / `output` names follow
[models.dev](https://models.dev), so its values can be copied across as
written.

An id containing a dot is a TOML dotted key, so quote it:
`[models."model-a.1"]`.

`auto_compact` is a profile choice, not a model fact. Set
`CLAUDE_CODE_AUTO_COMPACT_WINDOW` in `env` where a profile needs it.

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

### append prompts

`append_system_prompt` holds text that claude appends to its system prompt.
Unlike `env` and `args`, these texts accumulate along the `extends` chain:
every generation's text is kept, root first, and clx joins them with a blank
line into one `--append-system-prompt` flag.

```toml
[[profiles]]
name = "guard"
hidden = true
append_system_prompt = "Before adding complexity, explain the failure mode."

[[profiles]]
name = "work"
extends = "guard"
append_system_prompt = "You are the lead; delegate execution to subagents."
```

`clx work` launches with both texts, guard first. A child never replaces its
parents' prompt text.

The field takes the text itself, `{ file = "path" }`, or a list of those. A
relative path is resolved against the directory holding `config.toml`, so
prompts can live next to it:

```toml
append_system_prompt = [
  { file = "prompts/shared.md" },
  { file = "prompts/extra.md" },
]
```

A list lets one profile combine shared text with its own.

The `--append-system-prompt` and `--append-system-prompt-file` flags are
rejected in `args`, because claude reads each one once and a hand-written flag
would silently replace the field text. The picker preview shows the resolved
env and args, then a `parts` count for the append prompt; the text itself is
not shown.

### agents

`agents` defines subagents for the session. clx passes them through `--agents`,
which takes an inline JSON object; the `[[profiles.agents]]` table is the typed
form of that object.

```toml
[[profiles.agents]]
name = "worker"
description = "Execution hand for mechanical work"
model = { name = "example/small-model", id = "small-model" }
prompt = { file = "prompts/worker.md" }
tools = ["Read", "Edit", "Bash"]
permissionMode = "acceptEdits"
```

`name` is the agent's key. `prompt` and `model` work as elsewhere: the prompt is
inline text or `{ file }`, and the model is a name claude understands
(`sonnet`, `inherit`, a full id) or a `models` ref. Every other key is copied
into the JSON as written, so claude's own agent fields — `tools`,
`disallowedTools`, `permissionMode`, `effort`, `maxTurns`, `mcpServers`,
`hooks`, `skills`, `isolation`, and the rest — pass through.

Agents from the `extends` chain merge by name: a child's agent replaces the
parent's with the same name. `--agents` is rejected in `args`. The picker
preview lists the agent names.

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

# Print the full launch command without running claude
clx work --dry-run

# Print version
clx --version
```

`--dry-run` prints the env assignments and the argv exactly as they would be
executed, including the append prompt and `--agents` JSON. `$VAR` references
are expanded, so the output can contain secrets.

## How It Works

1. **Config loading** — reads `config.toml`, then `models.toml`, then
   `conf.d/*.toml`, merging as it goes
2. **Profile selection** — interactive fzf picker or CLI argument
3. **Inheritance resolution** — merges `[global]`, then walks the `extends`
   chain: env per key, args per flag, prompts and agents by name
4. **Launch** — expands `$VAR` in the env values, adds the append-prompt and
   `--agents` flags, removes every inherited `ANTHROPIC_*` / `CLAUDE_CODE_*`
   variable, and `execve`s into `claude`

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
