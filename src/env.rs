//! Env entries: the `$` template model, how an `env` table is read from the
//! config file, and expansion against clx's own environment.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Unexpected, Visitor};
use thiserror::Error;

/// A parsed `$` template. Parsing is the validation step — a malformed
/// reference is rejected here, where the caller can still attach its TOML line
/// number.
#[derive(Debug, Clone)]
pub struct Template {
    raw: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone)]
enum Segment {
    Literal(String),
    Var(String),
}

/// What the process environment had to say about one variable.
pub enum Lookup {
    Value(String),
    Missing,
    NotUnicode,
}

/// Failure to expand — always an authoring or environment error, never a silent
/// fallback.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExpandError {
    /// `$VAR` names a variable clx's own environment doesn't have.
    #[error("${0} is not set")]
    Missing(String),
    /// `$VAR` is set, but not to valid Unicode.
    #[error("${0} is set but not valid Unicode")]
    NotUnicode(String),
}

impl Template {
    /// Split a value into literal text and `$VAR` / `${VAR}` references, with
    /// `$$` as a literal `$`. A reference name matches `[A-Za-z_][A-Za-z0-9_]*`;
    /// anything else after a `$` is rejected rather than guessed at.
    pub fn parse(text: &str) -> Result<Self, String> {
        Ok(Self {
            raw: text.to_owned(),
            segments: scan(text)?,
        })
    }

    /// The template exactly as written — shown verbatim in the picker preview.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Expand against `lookup`, which reads clx's own environment.
    pub fn expand(&self, lookup: &impl Fn(&str) -> Lookup) -> Result<String, ExpandError> {
        let mut expanded = String::with_capacity(self.raw.len());
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => expanded.push_str(text),
                Segment::Var(name) => match lookup(name) {
                    Lookup::Value(value) => expanded.push_str(&value),
                    Lookup::Missing => return Err(ExpandError::Missing(name.clone())),
                    Lookup::NotUnicode => return Err(ExpandError::NotUnicode(name.clone())),
                },
            }
        }
        Ok(expanded)
    }
}

/// Deserialize an `env` table from the config file. Values must be TOML
/// scalars; each is parsed into a [`Template`] right here, inside the value's
/// own deserialization, because that is the only point where toml can still
/// attach the offending line to the error.
pub fn de_env_map<'de, D>(de: D) -> Result<BTreeMap<String, Template>, D::Error>
where
    D: Deserializer<'de>,
{
    struct EnvVisitor;

    impl<'de> Visitor<'de> for EnvVisitor {
        type Value = BTreeMap<String, Template>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a table of environment variables")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut env = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                let value = map.next_value_seed(EnvValue(&key))?;
                env.insert(key, value);
            }
            Ok(env)
        }
    }

    de.deserialize_map(EnvVisitor)
}

/// Seed for one env value. It carries the key because toml drops its own
/// ``in `a.b.c` `` context line whenever the error already has a span — which
/// ours always does — so the message has to name the variable itself.
struct EnvValue<'a>(&'a str);

impl<'de> DeserializeSeed<'de> for EnvValue<'_> {
    type Value = Template;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<Template, D::Error> {
        let key = self.0;
        let text = de
            .deserialize_any(ScalarVisitor)
            .map_err(|e| keyed_error(key, e))?;
        Template::parse(&text).map_err(|e| keyed_error(key, e))
    }
}

fn keyed_error<E: de::Error>(key: &str, detail: impl fmt::Display) -> E {
    de::Error::custom(format!("env.{key}: {detail}"))
}

/// Accepts the four TOML scalar kinds, stringifying each. Tables and datetimes
/// both arrive as maps here (toml deserializes a datetime through a one-entry
/// map), arrays as sequences — all rejected, each with a message that names
/// what actually showed up. Floats go through Rust's `Display`, so `1.0`
/// becomes `"1"`: env values are strings downstream, and reproducing TOML's
/// own spelling of a float is not something a config author can rely on.
struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            "a string, number, or boolean (tables, arrays, and datetimes are not env values)",
        )
    }

    fn visit_str<E>(self, v: &str) -> Result<String, E> {
        Ok(v.to_owned())
    }

    fn visit_bool<E>(self, v: bool) -> Result<String, E> {
        Ok(v.to_string())
    }

    fn visit_i64<E>(self, v: i64) -> Result<String, E> {
        Ok(v.to_string())
    }

    fn visit_f64<E>(self, v: f64) -> Result<String, E> {
        Ok(v.to_string())
    }

    fn visit_map<A: MapAccess<'de>>(self, _map: A) -> Result<String, A::Error> {
        Err(de::Error::invalid_type(Unexpected::Map, &self))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, _seq: A) -> Result<String, A::Error> {
        Err(de::Error::invalid_type(Unexpected::Seq, &self))
    }
}

/// The one scanner behind [`Template::parse`] and (via the segments) expansion.
fn scan(template: &str) -> Result<Vec<Segment>, String> {
    let bytes = template.as_bytes();
    let mut segments = Vec::new();
    let mut literal_start = 0;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if literal_start < i {
            segments.push(Segment::Literal(template[literal_start..i].to_owned()));
        }
        let rest = &template[i..]; // starts at an ASCII byte, so `[1..]` is safe
        if rest.starts_with("$$") {
            segments.push(Segment::Literal("$".to_owned()));
            i += 2;
        } else if let Some(body) = rest.strip_prefix("${") {
            let end = body.find('}').ok_or_else(|| "unclosed `${`".to_string())?;
            let name = &body[..end];
            if !is_name(name) {
                return Err(format!("`${{{name}}}` is not a valid variable reference"));
            }
            segments.push(Segment::Var(name.to_owned()));
            i += 2 + end + 1;
        } else {
            let len = name_prefix_len(&rest[1..]);
            if len == 0 {
                return Err("`$` must be followed by a variable name, `{`, or `$`".to_string());
            }
            segments.push(Segment::Var(rest[1..1 + len].to_owned()));
            i += 1 + len;
        }
        literal_start = i;
    }

    if literal_start < bytes.len() {
        segments.push(Segment::Literal(template[literal_start..].to_owned()));
    }
    Ok(segments)
}

fn is_name(name: &str) -> bool {
    name_prefix_len(name) == name.len() && !name.is_empty()
}

/// Length in bytes of the leading `[A-Za-z_][A-Za-z0-9_]*` run, `0` when there
/// isn't one. Counts ASCII bytes only, so the result is always a char boundary.
fn name_prefix_len(text: &str) -> usize {
    let mut len = 0;
    for byte in text.bytes() {
        let valid = if len == 0 {
            byte.is_ascii_alphabetic() || byte == b'_'
        } else {
            byte.is_ascii_alphanumeric() || byte == b'_'
        };
        if !valid {
            break;
        }
        len += 1;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Lookup {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |name| match map.get(name) {
            Some(value) => Lookup::Value(value.clone()),
            None => Lookup::Missing,
        }
    }

    fn expand(template: &str, lookup: &impl Fn(&str) -> Lookup) -> Result<String, ExpandError> {
        Template::parse(template).unwrap().expand(lookup)
    }

    #[test]
    fn expands_references_in_place() {
        let env = lookup(&[("FOO", "one"), ("BAR", "two")]);
        assert_eq!(expand("$FOO/$BAR!", &env).unwrap(), "one/two!");
        assert_eq!(expand("x${FOO}y", &env).unwrap(), "xoney");
        assert_eq!(expand("$FOO$BAR", &env).unwrap(), "onetwo");
    }

    #[test]
    fn plain_templates_come_back_untouched() {
        let env = lookup(&[]);
        assert_eq!(expand("", &env).unwrap(), "");
        assert_eq!(expand("nothing to do", &env).unwrap(), "nothing to do");
    }

    #[test]
    fn dollar_escape_is_a_literal_dollar() {
        let env = lookup(&[("FOO", "x")]);
        assert_eq!(expand("100$$", &env).unwrap(), "100$");
        assert_eq!(expand("$$FOO", &env).unwrap(), "$FOO");
    }

    #[test]
    fn missing_variable_is_an_error() {
        let env = lookup(&[]);
        assert_eq!(
            expand("a$NOPE", &env).unwrap_err(),
            ExpandError::Missing("NOPE".into())
        );
    }

    #[test]
    fn a_set_but_non_unicode_variable_is_reported_as_such() {
        assert_eq!(
            expand("$TOKEN", &|_| Lookup::NotUnicode).unwrap_err(),
            ExpandError::NotUnicode("TOKEN".into())
        );
    }

    #[test]
    fn malformed_references_are_rejected() {
        for bad in ["$", "$-", "$1", "${", "${}", "${A", "$é"] {
            assert!(Template::parse(bad).is_err(), "accepted {bad:?}");
        }
        for good in ["", "plain", "$FOO", "${FOO}", "$$", "a$B_c9", "${FOO}bar"] {
            assert!(Template::parse(good).is_ok(), "rejected {good:?}");
        }
    }
}
