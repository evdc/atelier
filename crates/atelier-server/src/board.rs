//! The board config a project authors in `.atelier/board.json`: which statuses exist and how they group into
//! columns. [`effective`] is pure: it turns the file's text into the config the board shows, plus one line per
//! thing it had to fix. AT-038 reuses it to validate writes.

use std::collections::{HashMap, HashSet};

use atelier_proto::{BoardColumn, BoardConfig};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The config's path, relative to the project root.
pub const PATH: &str = ".atelier/board.json";

/// What a project shows when it has no file (and what `.atelier/tickets/README.md` documents).
const DEFAULT_STATUSES: [&str; 3] = ["open", "in-progress", "done"];

pub fn defaults() -> BoardConfig {
    BoardConfig {
        statuses: DEFAULT_STATUSES.iter().map(|s| s.to_string()).collect(),
        columns: DEFAULT_STATUSES.iter().map(|s| column(s, &[s])).collect(),
        extra: Map::new(),
    }
}

fn column(title: &str, statuses: &[&str]) -> BoardColumn {
    BoardColumn {
        title: title.into(),
        statuses: statuses.iter().map(|s| s.to_string()).collect(),
        collapsed: false,
        extra: Map::new(),
    }
}

/// The config for `raw` (the file's text, or `None` if there is no file), and the problems found in it.
///
/// Lenient: the board is shown anyway. A column with no statuses is dropped; a status a column uses but
/// `statuses` lacks is appended to it; a status in two columns stays in the first; unparsable JSON gives
/// the defaults. A missing `columns` gives one column per status, a missing `statuses` the statuses the
/// columns use. Unknown keys are kept.
pub fn effective(raw: Option<&str>) -> (BoardConfig, Vec<String>) {
    let Some(raw) = raw else { return (defaults(), Vec::new()) };
    let file: File = match serde_json::from_str(raw) {
        Ok(f) => f,
        Err(e) => return (defaults(), vec![format!("not valid board config, using defaults: {e}")]),
    };
    if file.statuses.is_none() && file.columns.is_none() {
        return (BoardConfig { extra: file.extra, ..defaults() }, Vec::new());
    }
    let mut problems = Vec::new();
    let columns = match file.columns {
        Some(columns) => columns,
        None => file.statuses.iter().flatten().map(|s| column(s, &[s])).collect(),
    };
    let mut statuses = match file.statuses {
        Some(listed) => unique(listed, |s| problems.push(format!("status `{s}` is listed more than once in statuses"))),
        None => unique(columns.iter().flat_map(|c| c.statuses.clone()).collect(), |_| {}),
    };

    // Which column each status landed in: the first claim wins.
    let mut home: HashMap<String, String> = HashMap::new();
    let mut kept: Vec<BoardColumn> = Vec::new();
    for mut col in columns {
        let title = col.title.clone();
        let mut own = Vec::new();
        for s in unique(col.statuses, |s| {
            problems.push(format!("status `{s}` is listed more than once in column `{title}`"))
        }) {
            match home.get(&s) {
                Some(first) => problems.push(format!("status `{s}` is in more than one column; kept in `{first}`")),
                None => {
                    home.insert(s.clone(), title.clone());
                    own.push(s);
                }
            }
        }
        if own.is_empty() {
            problems.push(format!("column `{title}` has no statuses; dropped"));
            continue;
        }
        for s in &own {
            if !statuses.contains(s) {
                problems.push(format!("status `{s}` is used by column `{title}` but not listed in statuses; added"));
                statuses.push(s.clone());
            }
        }
        col.statuses = own;
        kept.push(col);
    }
    (BoardConfig { statuses, columns: kept, extra: file.extra }, problems)
}

/// The file as written, before validation. Both lists are optional so that absence can be told apart.
#[derive(Deserialize)]
struct File {
    statuses: Option<Vec<String>>,
    columns: Option<Vec<BoardColumn>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// `items` without repeats, reporting each repeat (in order) to `on_repeat`.
fn unique(items: Vec<String>, mut on_repeat: impl FnMut(&str)) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter_map(|s| {
            if seen.insert(s.clone()) {
                Some(s)
            } else {
                on_repeat(&s);
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(c: &BoardConfig) -> Vec<&str> {
        c.columns.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn no_file_gives_defaults() {
        let (c, problems) = effective(None);
        assert_eq!(c.statuses, ["open", "in-progress", "done"]);
        assert_eq!(titles(&c), ["open", "in-progress", "done"]);
        assert_eq!(c.columns[1].statuses, ["in-progress"]);
        assert!(!c.columns[0].collapsed);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_valid_file_is_used_as_written() {
        let raw = r#"{
            "statuses": ["open", "in-progress", "review", "done"],
            "columns": [
              { "title": "To do",  "statuses": ["open"] },
              { "title": "Doing",  "statuses": ["in-progress"] },
              { "title": "Review", "statuses": ["review"] },
              { "title": "Done",   "statuses": ["done"], "collapsed": true }
            ]
        }"#;
        let (c, problems) = effective(Some(raw));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(c.statuses, ["open", "in-progress", "review", "done"]);
        assert_eq!(titles(&c), ["To do", "Doing", "Review", "Done"]);
        assert!(c.columns[3].collapsed);
    }

    #[test]
    fn a_column_without_statuses_is_dropped() {
        let (c, problems) = effective(Some(
            r#"{"statuses": ["open"], "columns": [{"title": "Empty", "statuses": []}, {"title": "Open", "statuses": ["open"]}]}"#,
        ));
        assert_eq!(titles(&c), ["Open"]);
        assert_eq!(problems, ["column `Empty` has no statuses; dropped"]);
    }

    #[test]
    fn a_status_missing_from_statuses_is_appended() {
        let (c, problems) =
            effective(Some(r#"{"statuses": ["open"], "columns": [{"title": "A", "statuses": ["open", "review"]}]}"#));
        assert_eq!(c.statuses, ["open", "review"]);
        assert_eq!(problems, ["status `review` is used by column `A` but not listed in statuses; added"]);
    }

    #[test]
    fn a_status_in_two_columns_stays_in_the_first() {
        let raw = r#"{"statuses": ["open", "done"], "columns": [
            {"title": "A", "statuses": ["open", "done"]},
            {"title": "B", "statuses": ["done"]}]}"#;
        let (c, problems) = effective(Some(raw));
        assert_eq!(c.columns.len(), 1);
        assert_eq!(c.columns[0].statuses, ["open", "done"]);
        assert_eq!(
            problems,
            ["status `done` is in more than one column; kept in `A`", "column `B` has no statuses; dropped"]
        );
    }

    #[test]
    fn a_status_listed_twice_is_reported_once() {
        let (c, problems) =
            effective(Some(r#"{"statuses": ["open", "open"], "columns": [{"title": "A", "statuses": ["open"]}]}"#));
        assert_eq!(c.statuses, ["open"]);
        assert_eq!(problems, ["status `open` is listed more than once in statuses"]);
    }

    #[test]
    fn invalid_json_gives_defaults_and_a_problem() {
        let (c, problems) = effective(Some("{ not json"));
        assert_eq!(c.statuses, ["open", "in-progress", "done"]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("not valid board config, using defaults:"));
    }

    #[test]
    fn wrong_types_count_as_invalid_json() {
        let (c, problems) = effective(Some(r#"{"statuses": "open"}"#));
        assert_eq!(c.statuses, ["open", "in-progress", "done"]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn missing_columns_mean_one_per_status_and_missing_statuses_come_from_columns() {
        let (c, problems) = effective(Some(r#"{"statuses": ["open", "review"]}"#));
        assert_eq!(titles(&c), ["open", "review"]);
        assert!(problems.is_empty());

        let (c, problems) = effective(Some(r#"{"columns": [{"title": "Doing", "statuses": ["in-progress"]}]}"#));
        assert_eq!(c.statuses, ["in-progress"]);
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_fields_survive_parse_and_serialize() {
        let raw = r#"{"statuses": ["open"], "theme": "dark", "columns": [
            {"title": "A", "statuses": ["open"], "colour": "gold", "collapsed": false}]}"#;
        let (c, problems) = effective(Some(raw));
        assert!(problems.is_empty(), "{problems:?}");
        let out: Value = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["columns"][0]["colour"], "gold");
        assert_eq!(out["statuses"], serde_json::json!(["open"]));
        // And the parse of that output is the same config again.
        let (again, _) = effective(Some(&out.to_string()));
        assert_eq!(serde_json::to_value(&again).unwrap(), out);
    }
}
