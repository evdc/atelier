//! Stream names.
//!
//! Streams are hierarchical so that a prefix selects a subtree:
//!
//! ```text
//! global                    projects being added/removed, server-wide facts
//! p/<project>               project-level facts (files, tickets — later)
//! p/<project>/s/<session>   everything that happens in one session
//! ```
//!
//! Clients subscribe to an exact stream or a prefix (`p/<project>/`).

use crate::{ProjectId, SessionId};

pub const GLOBAL: &str = "global";

pub fn project(p: &ProjectId) -> String {
    format!("p/{p}")
}

pub fn session(p: &ProjectId, s: &SessionId) -> String {
    format!("p/{p}/s/{s}")
}

/// Parse `p/<project>/s/<session>`.
pub fn parse_session(stream: &str) -> Option<(ProjectId, SessionId)> {
    let rest = stream.strip_prefix("p/")?;
    let (p, s) = rest.split_once("/s/")?;
    Some((p.into(), s.into()))
}

/// The smallest string greater than every string with this prefix,
/// for index-friendly range scans (`stream >= prefix AND stream < end`).
pub fn prefix_end(prefix: &str) -> String {
    let mut bytes = prefix.as_bytes().to_vec();
    while let Some(last) = bytes.pop() {
        if last < 0xFF {
            bytes.push(last + 1);
            return String::from_utf8_lossy(&bytes).into_owned();
        }
    }
    String::from("\u{10FFFF}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_roundtrip() {
        let (p, s) = (ProjectId::generate(), SessionId::generate());
        assert_eq!(parse_session(&session(&p, &s)), Some((p, s)));
    }

    #[test]
    fn prefix_end_bounds() {
        assert_eq!(prefix_end("p/abc/"), "p/abc0");
        assert!("p/abc/s/x" < prefix_end("p/abc/").as_str());
    }
}

/// Which streams a subscription covers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StreamFilter {
    /// Exactly one stream.
    Stream(String),
    /// Every stream starting with this prefix (e.g. `p/<project>/`).
    Prefix(String),
}

impl StreamFilter {
    pub fn matches(&self, stream: &str) -> bool {
        match self {
            StreamFilter::Stream(s) => s == stream,
            StreamFilter::Prefix(p) => stream.starts_with(p.as_str()),
        }
    }
}
