//! Quoting for the shell snippets clx prints.

/// Wrap `s` in single quotes for safe embedding in a POSIX shell command.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
