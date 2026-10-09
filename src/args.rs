//! Merging of `args` across the `[global]` table, the `extends` chain, and the
//! command-line passthrough.
//!
//! Tokens are grouped into items with the help of [`crate::claude_flags`]; this
//! module owns what happens between generations: a replaceable flag takes its
//! key over wholesale, list flags accumulate, opaque tokens append.

use crate::claude_flags::Kind;

/// One argument group: a flag plus the tokens that belong to it, or a single
/// opaque token.
#[derive(Debug, Clone)]
struct Item {
    /// Merge key for known flags. `None` for anything opaque (positionals,
    /// unknown flags) — those are only ever appended.
    key: Option<String>,
    tokens: Vec<String>,
    /// Whether a later generation mentioning this key takes the item over.
    replaceable: bool,
}

impl Item {
    fn opaque(token: &str) -> Self {
        Self {
            key: None,
            tokens: vec![token.to_owned()],
            replaceable: false,
        }
    }
}

/// The resolved argument list — see the module docs for the merge rules.
#[derive(Debug, Clone, Default)]
pub struct ArgList(Vec<Item>);

impl ArgList {
    /// Overlay the next generation (`[global]`, one `extends` ancestor, or the
    /// command-line passthrough) onto the list so far.
    pub fn merge(&mut self, tokens: &[String]) {
        let items = parse(tokens);

        // A replaceable flag mentioned by this generation takes its key over
        // wholesale: every inherited item with that key goes, however many.
        // Items within one generation never take each other over.
        let taken: Vec<&str> = items
            .iter()
            .filter(|item| item.replaceable)
            .filter_map(|item| item.key.as_deref())
            .collect();
        self.0.retain(|item| match item.key.as_deref() {
            Some(key) => !taken.contains(&key),
            None => true,
        });

        // New items land at the tail, before an inherited `--` if there is one:
        // a later generation's flags must stay flags, not become positional
        // text after the separator.
        let at = self
            .0
            .iter()
            .position(|item| item.key.is_none() && item.tokens == ["--"])
            .unwrap_or(self.0.len());
        self.0.splice(at..at, items);
    }

    /// The final argv: everything merged so far, with `extra` overlaid the same
    /// way — the command line always wins over the config.
    pub fn merged_argv(&self, extra: &[String]) -> Vec<String> {
        let mut merged = self.clone();
        merged.merge(extra);
        merged.0.into_iter().flat_map(|item| item.tokens).collect()
    }
}

/// Split raw tokens into items. Known flags take the tokens their kind says;
/// everything else becomes a single opaque item.
fn parse(tokens: &[String]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        let flag = token
            .split_once('=')
            .map_or(token.as_str(), |(name, _)| name);
        let Some(kind) = Kind::of(flag) else {
            items.push(Item::opaque(token));
            i += 1;
            continue;
        };

        // `--flag=value` carries its value in the same token.
        let take = if flag.len() < token.len() {
            0
        } else {
            match kind {
                Kind::Switch => 0,
                Kind::Value | Kind::Repeatable => 1,
                Kind::Optional => usize::from(tokens.get(i + 1).is_some_and(|t| is_value(t))),
                Kind::Variadic => tokens[i + 1..].iter().take_while(|t| is_value(t)).count(),
            }
        };
        let end = (i + 1 + take).min(tokens.len());
        items.push(Item {
            key: Some(flag.to_owned()),
            tokens: tokens[i..end].to_vec(),
            replaceable: kind.takes_over(),
        });
        i = end;
    }
    items
}

fn is_value(token: &str) -> bool {
    !token.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Merge each generation in order and return the final argv.
    fn argv(generations: &[&[&str]]) -> Vec<String> {
        let mut list = ArgList::default();
        for generation in generations {
            list.merge(&strs(generation));
        }
        list.merged_argv(&[])
    }

    #[test]
    fn later_generation_takes_over_the_same_flag() {
        assert_eq!(
            argv(&[
                &["--dangerously-skip-permissions", "--model", "opus"],
                &["--model=sonnet"],
            ]),
            strs(&["--dangerously-skip-permissions", "--model=sonnet"])
        );
        assert_eq!(
            argv(&[&["--verbose"], &["--verbose"]]),
            strs(&["--verbose"])
        );
    }

    #[test]
    fn takeover_drops_every_inherited_duplicate() {
        assert_eq!(
            argv(&[
                &["--model", "opus", "--model", "sonnet"],
                &["--model", "haiku"],
            ]),
            strs(&["--model", "haiku"])
        );
    }

    #[test]
    fn a_shorter_takeover_does_not_capture_the_next_token() {
        // The takeover lands at the tail, so the inherited positional stays a
        // positional instead of becoming `--resume`'s value.
        assert_eq!(
            argv(&[
                &["--resume=sess-123", "continue the refactor"],
                &["--resume"],
            ]),
            strs(&["continue the refactor", "--resume"])
        );
    }

    #[test]
    fn later_items_land_before_an_inherited_separator() {
        assert_eq!(
            argv(&[&["--model", "opus", "--", "do the thing"], &["--verbose"],]),
            strs(&["--model", "opus", "--verbose", "--", "do the thing"])
        );
    }

    #[test]
    fn unknown_tokens_are_appended() {
        assert_eq!(
            argv(&[&["--model=opus"], &["fix the bug", "--not-a-real-flag"]]),
            strs(&["--model=opus", "fix the bug", "--not-a-real-flag"])
        );
    }

    #[test]
    fn duplicates_within_one_generation_survive() {
        assert_eq!(
            argv(&[&["--add-dir", "/a", "--add-dir", "/b"]]),
            strs(&["--add-dir", "/a", "--add-dir", "/b"])
        );
        assert_eq!(
            argv(&[&["--mystery", "--mystery"]]),
            strs(&["--mystery", "--mystery"])
        );
    }

    #[test]
    fn optional_value_only_takes_a_non_flag_token() {
        assert_eq!(
            argv(&[&["--resume=old"], &["--resume", "abc"]]),
            strs(&["--resume", "abc"])
        );
        assert_eq!(
            argv(&[&["--resume=old"], &["--resume", "--verbose"]]),
            strs(&["--resume", "--verbose"])
        );
    }

    #[test]
    fn variadic_flags_take_values_greedily_and_accumulate() {
        assert_eq!(
            argv(&[&["--add-dir", "/a", "/b", "--verbose"]]),
            strs(&["--add-dir", "/a", "/b", "--verbose"])
        );
        assert_eq!(
            argv(&[&["--add-dir=/a"], &["--add-dir=/b", "--add-dir=/c"]]),
            strs(&["--add-dir=/a", "--add-dir=/b", "--add-dir=/c"])
        );
    }

    #[test]
    fn repeatable_flags_accumulate_across_generations() {
        assert_eq!(
            argv(&[&["--plugin-dir", "a"], &["--plugin-dir", "b"]]),
            strs(&["--plugin-dir", "a", "--plugin-dir", "b"])
        );
    }

    #[test]
    fn a_valueless_value_flag_still_merges_by_name() {
        assert_eq!(argv(&[&["--model=opus"], &["--model"]]), strs(&["--model"]));
    }

    #[test]
    fn passthrough_overwrites_the_config() {
        let mut list = ArgList::default();
        list.merge(&strs(&["--dangerously-skip-permissions", "--model=opus"]));
        assert_eq!(
            list.merged_argv(&strs(&["--model", "sonnet", "extra"])),
            strs(&[
                "--dangerously-skip-permissions",
                "--model",
                "sonnet",
                "extra"
            ])
        );
    }
}
