//! Front matter of a markdown file: a `---` fenced block of `key: value` lines at the very top.
//!
//! Only the subset our tickets use: bare scalars, `"double"`/`'single'` quoted strings, and inline lists
//! `[a, b]`. Pure and total: malformed input yields a `problem`, never a panic.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    List(Vec<String>),
}

impl Value {
    /// A list as itself; a scalar as a one-item list (empty if blank).
    pub fn into_list(self) -> Vec<String> {
        match self {
            Value::List(l) => l,
            Value::Scalar(s) if s.is_empty() => vec![],
            Value::Scalar(s) => vec![s],
        }
    }
    /// A scalar as itself; a list re-joined with `, `.
    pub fn into_text(self) -> String {
        match self {
            Value::Scalar(s) => s,
            Value::List(l) => l.join(", "),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FrontMatter<'a> {
    /// Keys in file order are not kept; a repeated key keeps its last value.
    pub fields: BTreeMap<String, Value>,
    /// Everything after the closing fence (the whole text if there is no front matter).
    pub body: &'a str,
    /// What was wrong, if anything. Parsing carries on past bad lines, so `fields` is still best-effort.
    pub problem: Option<String>,
}

pub fn parse(text: &str) -> FrontMatter<'_> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(trim_eol) != Some("---") {
        return FrontMatter { body: text, ..Default::default() };
    }
    let mut fm = FrontMatter::default();
    let mut offset = text.split_inclusive('\n').next().map_or(0, str::len);
    let mut problems: Vec<String> = vec![];
    for (i, raw) in lines.enumerate() {
        offset += raw.len();
        let line = trim_eol(raw);
        if line.trim_end() == "---" {
            fm.body = &text[offset..];
            fm.problem = join(problems);
            return fm;
        }
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match line.split_once(':') {
            Some((k, v)) if is_key(k.trim()) => match value(v.trim()) {
                Ok(v) => {
                    fm.fields.insert(k.trim().to_string(), v);
                }
                Err(e) => problems.push(format!("front matter line {}: {e}", i + 2)),
            },
            _ => problems.push(format!("front matter line {}: expected `key: value`", i + 2)),
        }
    }
    // No closing fence: don't guess where the body starts.
    problems.push("front matter is not closed with `---`".into());
    FrontMatter { fields: fm.fields, body: text, problem: join(problems) }
}

fn join(p: Vec<String>) -> Option<String> {
    (!p.is_empty()).then(|| p.join("; "))
}

fn trim_eol(l: &str) -> &str {
    l.strip_suffix('\n').map_or(l, |l| l.strip_suffix('\r').unwrap_or(l))
}

fn is_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn value(v: &str) -> Result<Value, String> {
    if let Some(inner) = v.strip_prefix('[') {
        let inner = inner.strip_suffix(']').ok_or("list is missing `]`")?;
        return Ok(Value::List(split_list(inner)?));
    }
    scalar(v).map(Value::Scalar)
}

fn scalar(v: &str) -> Result<String, String> {
    match v.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let (s, rest) = quoted(v, q)?;
            if rest.trim().is_empty() || rest.trim_start().starts_with('#') {
                Ok(s)
            } else {
                Err("text after closing quote".into())
            }
        }
        _ => Ok(v.to_string()),
    }
}

/// Reads one quoted string off the front of `s` (which starts with `q`); returns it and the remainder.
fn quoted(s: &str, q: char) -> Result<(String, &str), String> {
    let mut out = String::new();
    let mut it = s.char_indices().skip(1);
    while let Some((i, c)) = it.next() {
        match c {
            '\\' if q == '"' => match it.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, e)) => out.push(e),
                None => break,
            },
            // YAML single quotes escape by doubling.
            '\'' if q == '\'' && s[i + 1..].starts_with('\'') => {
                out.push('\'');
                it.next();
            }
            c if c == q => return Ok((out, &s[i + 1..])),
            c => out.push(c),
        }
    }
    Err("unterminated quote".into())
}

fn split_list(inner: &str) -> Result<Vec<String>, String> {
    let mut items = vec![];
    let mut rest = inner.trim();
    while !rest.is_empty() {
        let (item, tail) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let (s, tail) = quoted(rest, q)?;
                let tail = tail.trim_start();
                if !(tail.is_empty() || tail.starts_with(',')) {
                    return Err("text after closing quote".into());
                }
                (s, tail)
            }
            _ => match rest.split_once(',') {
                Some((a, _)) => (a.trim().to_string(), &rest[a.len()..]),
                None => (rest.trim().to_string(), ""),
            },
        };
        if !item.is_empty() {
            items.push(item);
        }
        rest = tail.strip_prefix(',').unwrap_or(tail).trim_start();
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::Scalar(v.into())
    }
    fn l(v: &[&str]) -> Value {
        Value::List(v.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn scalars_quotes_and_lists() {
        let fm = parse(
            "---\nid: AT-1\ntitle: \"`ask`: synchronous \\\"questions\\\"\"\nq: 'it''s'\narea: server, proto\n\
             depends: [AT-2, AT-3]\nnone: []\nquoted: [\"a, b\", c]\nempty:\n---\n# Body\n",
        );
        assert_eq!(fm.problem, None);
        assert_eq!(fm.fields["id"], s("AT-1"));
        assert_eq!(fm.fields["title"], s("`ask`: synchronous \"questions\""));
        assert_eq!(fm.fields["q"], s("it's"));
        assert_eq!(fm.fields["area"], s("server, proto"));
        assert_eq!(fm.fields["depends"], l(&["AT-2", "AT-3"]));
        assert_eq!(fm.fields["none"], l(&[]));
        assert_eq!(fm.fields["quoted"], l(&["a, b", "c"]));
        assert_eq!(fm.fields["empty"], s(""));
        assert_eq!(fm.body, "# Body\n");
    }

    #[test]
    fn no_front_matter() {
        let fm = parse("# Just a note\n");
        assert_eq!((fm.fields.len(), fm.body, fm.problem), (0, "# Just a note\n", None));
        assert_eq!(parse("").body, "");
        // A fence that isn't on the first line is just text.
        assert_eq!(parse("\n---\na: b\n---\n").fields.len(), 0);
    }

    #[test]
    fn unterminated_fence() {
        let fm = parse("---\nid: AT-1\ntitle: x\n");
        assert!(fm.problem.unwrap().contains("not closed"));
        assert_eq!(fm.fields["id"], s("AT-1"));
        assert_eq!(fm.body, "---\nid: AT-1\ntitle: x\n");
    }

    #[test]
    fn bad_lines_are_reported_and_skipped() {
        let fm = parse("---\nid: AT-1\nnonsense\ntitle: \"open\nsize: M\ndepends: [a\n---\nbody");
        let p = fm.problem.unwrap();
        assert!(p.contains("line 3") && p.contains("line 4") && p.contains("line 6"), "{p}");
        assert_eq!((fm.fields["id"].clone(), fm.fields["size"].clone()), (s("AT-1"), s("M")));
        assert_eq!(fm.body, "body");
    }

    #[test]
    fn crlf_and_bom() {
        let fm = parse("\u{feff}---\r\nid: AT-1\r\ntitle: \"T\"\r\ndepends: [A, B]\r\n---\r\nbody\r\n");
        assert_eq!(fm.problem, None);
        assert_eq!(fm.fields["id"], s("AT-1"));
        assert_eq!(fm.fields["title"], s("T"));
        assert_eq!(fm.fields["depends"], l(&["A", "B"]));
        assert_eq!(fm.body, "body\r\n");
    }

    #[test]
    fn comments_blank_lines_and_trailing_comment() {
        let fm = parse("---\n# note\n\nsize: M\nt: \"x\" # why\n---\n");
        assert_eq!(fm.problem, None);
        assert_eq!(fm.fields["t"], s("x"));
    }
}
