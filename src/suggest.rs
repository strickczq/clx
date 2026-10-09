//! "Did you mean" diagnostics for the config file.

/// Format a TOML deserialize error, appending a did-you-mean hint when serde
/// rejected an unknown field that is close to a known one.
pub fn format_toml_error(err: toml::de::Error) -> String {
    let mut text = err.to_string();
    let hint = unknown_field_hint(err.message()).or_else(|| unknown_field_hint(&text));
    if let Some(hint) = hint {
        text.push('\n');
        text.push_str(&hint);
    }
    text
}

/// Closest candidate by Levenshtein distance, if close enough to be a typo.
pub fn closest<'a>(
    unknown: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|candidate| (levenshtein(candidate, unknown), candidate))
        .filter(|(d, candidate)| *d > 0 && *d <= candidate.len().max(unknown.len()) / 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, candidate)| candidate)
}

/// Parse serde's `unknown field` message and, if a close known field exists,
/// return `did you mean \`field\`?`.
fn unknown_field_hint(msg: &str) -> Option<String> {
    let (unknown, expected) = parse_unknown_field_error(msg)?;
    let hint = closest(unknown, expected)?;
    Some(format!("did you mean `{hint}`?"))
}

/// Pull `(unknown, expected_fields)` out of serde's unknown-field wording:
/// - `unknown field \`foo\`, expected \`bar\``
/// - `unknown field \`foo\`, expected \`bar\` or \`baz\``
/// - `unknown field \`foo\`, expected one of \`a\`, \`b\`, \`c\``
fn parse_unknown_field_error(msg: &str) -> Option<(&str, Vec<&str>)> {
    let rest = msg.split("unknown field `").nth(1)?;
    let (unknown, rest) = rest.split_once('`')?;
    let rest = rest.trim_start_matches([' ', ',']);
    let rest = rest.strip_prefix("expected ")?;
    if rest.starts_with("there are no fields") {
        return None;
    }
    let expected = if let Some(list) = rest.strip_prefix("one of ") {
        parse_backticked_list(list)
    } else if let Some((a, b)) = rest.split_once("` or `") {
        vec![a.trim_matches('`'), b.trim_matches('`')]
    } else {
        vec![rest.trim_matches('`').trim_end_matches(['.', ' '])]
    };
    let expected: Vec<&str> = expected.into_iter().filter(|s| !s.is_empty()).collect();
    (!expected.is_empty()).then_some((unknown, expected))
}

fn parse_backticked_list(list: &str) -> Vec<&str> {
    list.split("`, `")
        .map(|s| s.trim().trim_matches('`').trim_end_matches(['.', ' ']))
        .collect()
}

/// Plain Levenshtein edit distance — small, no external dependency.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }

    #[test]
    fn closest_finds_a_typo_but_not_garbage() {
        let names = ["work", "personal"];
        assert_eq!(closest("wrok", names), Some("work"));
        assert_eq!(closest("zzzzzzzz", names), None);
    }

    #[test]
    fn parse_unknown_field_error_handles_serde_wordings() {
        let one = parse_unknown_field_error("unknown field `foo`, expected `bar`").unwrap();
        assert_eq!(one.0, "foo");
        assert_eq!(one.1, ["bar"]);

        let two =
            parse_unknown_field_error("unknown field `foo`, expected `bar` or `baz`").unwrap();
        assert_eq!(two.0, "foo");
        assert_eq!(two.1, ["bar", "baz"]);

        let many = parse_unknown_field_error(
            "unknown field `arg`, expected one of `name`, `extends`, `description`, `env`, `args`",
        )
        .unwrap();
        assert_eq!(many.0, "arg");
        assert_eq!(many.1, ["name", "extends", "description", "env", "args"]);
        assert_eq!(closest(many.0, many.1), Some("args"));
    }
}
