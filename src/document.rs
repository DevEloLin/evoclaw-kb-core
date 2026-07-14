//! The generic document model and the data-source plug-in surface.

/// A document ready for indexing, tagged with the source it came from.
#[derive(Debug, Clone)]
pub struct Document {
    /// Which source produced this (e.g. "obsidian", "feishu").
    pub source: String,
    /// Source-local unique id (e.g. vault-relative path, Feishu doc token).
    pub id: String,
    pub title: String,
    /// Plain text used for full-text indexing.
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub headings: Vec<String>,
    /// Freshness marker (mtime, edit-time, ...). Drives incremental refresh.
    pub version: u64,
}

/// NUL joins source+id into the index's unique key. NUL can't appear in a
/// path or token, so it can't collide with real ids.
pub const UID_SEP: char = '\u{0}';

impl Document {
    /// Index-wide unique key: `"{source}\0{id}"`. Used as the delete/upsert
    /// term (tantivy `delete_term` matches a single term, so the key must be
    /// one field, not a composite of two).
    pub fn uid(&self) -> String {
        make_uid(&self.source, &self.id)
    }
}

/// Build the unique key from a source id and a source-local id.
pub fn make_uid(source: &str, id: &str) -> String {
    format!("{source}{UID_SEP}{id}")
}

/// A pluggable source of documents. Implementors do all source-specific I/O.
///
/// Contract for incremental refresh:
/// - [`list`](DocSource::list) is the cheap "what exists now" enumeration.
///   Returning `Err` means the source is unavailable; the engine SKIPS it
///   (leaves its existing index untouched) rather than treating the docs as
///   deleted.
/// - [`load`](DocSource::load) is called only for new/changed ids.
/// - [`read`](DocSource::read) fetches full content for `kb_read`.
pub trait DocSource: Send + Sync {
    /// Stable source identifier, e.g. "obsidian" or "feishu". Must be unique
    /// across the sources handed to one index.
    fn source_id(&self) -> &str;

    /// Cheap listing of every current document as `(id, version)`.
    fn list(&self) -> eyre::Result<Vec<(String, u64)>>;

    /// Load and parse one document by its source-local id.
    fn load(&self, id: &str) -> eyre::Result<Document>;

    /// Read full content for `kb_read`; optional `section` narrows to a
    /// heading's slice when the source supports it.
    fn read(&self, id: &str, section: Option<&str>) -> eyre::Result<String>;
}
