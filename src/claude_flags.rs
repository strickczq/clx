//! What clx knows about claude's CLI: which tokens form one flag and whether a
//! later generation takes the key over or accumulates with it.
//!
//! This is the one place coupled to upstream, and the coupling degrades safely:
//! a flag missing from the table is appended verbatim, so a stale table loses
//! merging, never correctness. A *wrong* entry does misgroup tokens, so only
//! spellings taken straight from `claude --help` go in.

/// How a flag consumes tokens and merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// No value: `--verbose`.
    Switch,
    /// Exactly one value: `--model opus` or `--model=opus`.
    Value,
    /// A value only when the next token doesn't look like a flag:
    /// `--resume`, `--resume abc`.
    Optional,
    /// One or more values, greedily until the next `-` token:
    /// `--add-dir a b`.
    Variadic,
    /// One value per occurrence; occurrences accumulate rather than replace:
    /// `--plugin-dir a --plugin-dir b`.
    Repeatable,
}

impl Kind {
    /// Look up a flag name — as written, without an `=value` part. `None` for
    /// anything claude's help doesn't spell out here. Aliases are separate
    /// names, not merged: `-p` and `--print` never pair up.
    pub fn of(flag: &str) -> Option<Self> {
        Some(match flag {
            "--allow-dangerously-skip-permissions"
            | "--ax-screen-reader"
            | "--background"
            | "--bg"
            | "--bare"
            | "--brief"
            | "--chrome"
            | "--continue"
            | "-c"
            | "--dangerously-skip-permissions"
            | "--desktop"
            | "--disable-slash-commands"
            | "--exclude-dynamic-system-prompt-sections"
            | "--fork-session"
            | "--forward-subagent-text"
            | "--help"
            | "-h"
            | "--ide"
            | "--include-hook-events"
            | "--include-partial-messages"
            | "--no-chrome"
            | "--no-session-persistence"
            | "--print"
            | "-p"
            | "--replay-user-messages"
            | "--restricted"
            | "--safe-mode"
            | "--strict-mcp-config"
            | "--tmux"
            | "--verbose"
            | "--version"
            | "-v" => Self::Switch,

            "--agent"
            | "--agents"
            | "--append-system-prompt"
            | "--autocompact"
            | "--debug-file"
            | "--effort"
            | "--environment"
            | "--fallback-model"
            | "--input-format"
            | "--json-schema"
            | "--max-budget-usd"
            | "--model"
            | "--name"
            | "-n"
            | "--output-format"
            | "--permission-mode"
            | "--permission-prompts"
            | "--remote-control-session-name-prefix"
            | "--session-id"
            | "--setting-sources"
            | "--settings"
            | "--system-prompt"
            | "--system-prompt-snapshot" => Self::Value,

            "--cloud"
            | "--debug"
            | "-d"
            | "--from-pr"
            | "--prompt-suggestions"
            | "--remote-control"
            | "--resume"
            | "-r"
            | "--teleport"
            | "--worktree"
            | "-w" => Self::Optional,

            "--add-dir" | "--allowed-tools" | "--allowedTools" | "--betas"
            | "--disallowed-tools" | "--disallowedTools" | "--file" | "--mcp-config"
            | "--tools" => Self::Variadic,

            "--plugin-dir" | "--plugin-url" => Self::Repeatable,

            _ => return None,
        })
    }

    /// Whether this kind takes over a key the earlier generations used. List
    /// kinds accumulate instead — a child's `--add-dir` means "also".
    pub fn takes_over(self) -> bool {
        matches!(self, Self::Switch | Self::Value | Self::Optional)
    }
}
