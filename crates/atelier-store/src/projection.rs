use atelier_core::Event;
use rusqlite::Connection;

/// A read model folded from the log into its own SQLite tables.
///
/// Implementations run on the writer thread, inside the append transaction.
/// They must be deterministic functions of the events they see (no clock, no
/// IO besides their own tables) so that a rebuild gives the same result.
pub trait Projection: Send + 'static {
    /// Unique, stable name. Used as the key in the `projections` table.
    fn name(&self) -> &'static str;

    /// Bump to rebuild from scratch (schema or fold logic changed).
    fn version(&self) -> u32;

    /// Create tables/indexes (`IF NOT EXISTS`).
    fn migrate(&self, db: &Connection) -> rusqlite::Result<()>;

    /// Drop everything `migrate` created.
    fn reset(&self, db: &Connection) -> rusqlite::Result<()>;

    /// Fold one event. Record every row you insert/update/delete in `out`, so
    /// live views can push it to clients. Events of unknown kinds must be
    /// ignored; malformed bodies should be logged and skipped, not returned
    /// as errors (an error aborts the append).
    fn apply(&mut self, db: &Connection, ev: &Event, out: &mut Vec<Change>) -> anyhow::Result<()>;
}

/// "Row `key` of view `view` changed." Clients watching that view (and
/// `scope`, if they narrowed to one) re-read the row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Change {
    pub view: &'static str,
    pub key: String,
    /// Partition the row belongs to (e.g. its project), for scoped watches.
    pub scope: Option<String>,
}

impl Change {
    pub fn new(view: &'static str, key: impl Into<String>, scope: Option<String>) -> Self {
        Self { view, key: key.into(), scope }
    }
}
