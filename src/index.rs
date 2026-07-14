//! The unified tantivy index: schema, jieba tokenizer, and per-source
//! incremental refresh.
//!
//! Freshness is tracked in a nested manifest `{ source: { id: version } }`
//! next to the index, so refresh never enumerates stored documents and each
//! source's state is isolated.

use std::collections::HashMap;
use std::path::Path;

use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, FAST, STORED, STRING,
};
use tantivy::tokenizer::{LowerCaser, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::document::{make_uid, DocSource, Document};
use crate::error::Result;

const WRITER_HEAP_BYTES: usize = 50_000_000;
/// jieba (Chinese word segmentation) + lowercasing for ASCII.
pub(crate) const TOKENIZER: &str = "jieba";

/// Per-source, per-id version map: `{ source: { id: version } }`.
type Manifest = HashMap<String, HashMap<String, u64>>;

#[derive(Clone, Copy)]
pub(crate) struct Fields {
    pub uid: Field,
    pub source: Field,
    pub id: Field,
    pub title: Field,
    pub body: Field,
    pub tags: Field,
    pub headings: Field,
    pub version: Field,
}

pub struct KbIndex {
    pub(crate) index: Index,
    pub(crate) fields: Fields,
    pub(crate) reader: IndexReader,
    manifest_path: std::path::PathBuf,
}

/// Outcome of one [`KbIndex::sync`], aggregated across sources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RefreshStats {
    pub upserted: usize,
    pub removed: usize,
    /// Documents whose `load` failed (left un-indexed, retried next sync).
    pub failed: usize,
    /// Sources whose `list` failed (skipped, index left untouched).
    pub degraded: usize,
}

fn build_schema() -> (Schema, Fields) {
    let indexing = TextFieldIndexing::default()
        .set_tokenizer(TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text_stored = TextOptions::default()
        .set_indexing_options(indexing.clone())
        .set_stored();
    let text_only = TextOptions::default().set_indexing_options(indexing);

    let mut b = Schema::builder();
    // STRING = untokenized. `uid` is the single delete/upsert key.
    let uid = b.add_text_field("uid", STRING | STORED);
    let source = b.add_text_field("source", STRING | STORED);
    let id = b.add_text_field("id", STRING | STORED);
    let title = b.add_text_field("title", text_stored.clone());
    let body = b.add_text_field("body", text_stored); // STORED for snippets
    let tags = b.add_text_field("tags", STRING | STORED);
    let headings = b.add_text_field("headings", text_only);
    let version = b.add_u64_field("version", STORED | FAST);
    let schema = b.build();
    (
        schema,
        Fields {
            uid,
            source,
            id,
            title,
            body,
            tags,
            headings,
            version,
        },
    )
}

fn register_tokenizers(index: &Index) {
    let analyzer = TextAnalyzer::builder(tantivy_jieba::JiebaTokenizer::new())
        .filter(LowerCaser)
        .build();
    index.tokenizers().register(TOKENIZER, analyzer);
}

impl KbIndex {
    /// Open (or create) the index under `cache_dir`. Does not refresh — call
    /// [`sync`](KbIndex::sync) (typically in a background task) to populate.
    pub fn open(cache_dir: &Path) -> Result<Self> {
        let index_dir = cache_dir.join("index");
        std::fs::create_dir_all(&index_dir)?;

        let (schema, fields) = build_schema();
        let dir = tantivy::directory::MmapDirectory::open(&index_dir)?;
        let index = Index::open_or_create(dir, schema)?;
        register_tokenizers(&index);
        let reader = index.reader()?;

        Ok(KbIndex {
            index,
            fields,
            reader,
            manifest_path: cache_dir.join("manifest.json"),
        })
    }

    /// Bring the index in sync with all `sources`. Each source is diffed
    /// against its own manifest partition:
    /// - a source whose `list()` fails is skipped (its index is left intact);
    /// - a document whose `load()` fails is left un-indexed and NOT recorded,
    ///   so the next sync retries it.
    pub fn sync(&self, sources: &[std::sync::Arc<dyn DocSource>]) -> Result<RefreshStats> {
        let manifest = self.load_manifest()?;
        let mut next = manifest.clone();
        let mut writer: IndexWriter = self.index.writer(WRITER_HEAP_BYTES)?;
        let mut stats = RefreshStats::default();
        let mut dirty = false;

        for source in sources {
            let sid = source.source_id();
            let listed = match source.list() {
                Ok(l) => l,
                Err(e) => {
                    // CRITICAL: never treat an unavailable source as "all deleted".
                    tracing::warn!(source = sid, error = %e, "source list failed; skipping (index left intact)");
                    stats.degraded += 1;
                    continue;
                }
            };
            let current: HashMap<String, u64> = listed.into_iter().collect();
            let old = manifest.get(sid).cloned().unwrap_or_default();
            let mut recorded = current.clone();

            for (id, ver) in &current {
                if old.get(id) == Some(ver) {
                    continue; // unchanged
                }
                match source.load(id) {
                    Ok(doc) => {
                        let uid = doc.uid();
                        writer.delete_term(Term::from_field_text(self.fields.uid, &uid));
                        writer.add_document(self.to_tantivy(&doc))?;
                        stats.upserted += 1;
                        dirty = true;
                    }
                    Err(e) => {
                        tracing::warn!(source = sid, id = id, error = %e, "load failed; leaving doc un-indexed");
                        stats.failed += 1;
                        // Don't record it, so the next sync retries.
                        recorded.remove(id);
                    }
                }
            }

            for id in old.keys() {
                if !current.contains_key(id) {
                    writer.delete_term(Term::from_field_text(
                        self.fields.uid,
                        &make_uid(sid, id),
                    ));
                    stats.removed += 1;
                    dirty = true;
                }
            }

            next.insert(sid.to_string(), recorded);
        }

        if dirty {
            writer.commit()?;
            self.reader.reload()?;
        }
        self.save_manifest(&next)?;
        Ok(stats)
    }

    /// Number of documents currently visible to searches. `0` means the
    /// index is empty (first run, before any successful sync).
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    fn to_tantivy(&self, doc: &Document) -> TantivyDocument {
        let f = &self.fields;
        let mut d = TantivyDocument::default();
        d.add_text(f.uid, doc.uid());
        d.add_text(f.source, &doc.source);
        d.add_text(f.id, &doc.id);
        d.add_text(f.title, &doc.title);
        d.add_text(f.body, &doc.body);
        for t in &doc.tags {
            d.add_text(f.tags, t);
        }
        if !doc.headings.is_empty() {
            d.add_text(f.headings, doc.headings.join(" "));
        }
        d.add_u64(f.version, doc.version);
        d
    }

    fn load_manifest(&self) -> Result<Manifest> {
        match std::fs::read_to_string(&self.manifest_path) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        std::fs::write(&self.manifest_path, serde_json::to_vec(manifest)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::sync::{Arc, Mutex};

    /// In-memory source for engine tests. `fail_list` flips to simulate an
    /// unavailable source.
    struct MockSource {
        id: String,
        docs: Mutex<Vec<Document>>,
        fail_list: Mutex<bool>,
    }

    impl MockSource {
        fn new(id: &str, docs: Vec<Document>) -> Self {
            Self {
                id: id.into(),
                docs: Mutex::new(docs),
                fail_list: Mutex::new(false),
            }
        }
    }

    impl DocSource for MockSource {
        fn source_id(&self) -> &str {
            &self.id
        }
        fn list(&self) -> eyre::Result<Vec<(String, u64)>> {
            if *self.fail_list.lock().unwrap() {
                eyre::bail!("simulated source outage");
            }
            Ok(self
                .docs
                .lock()
                .unwrap()
                .iter()
                .map(|d| (d.id.clone(), d.version))
                .collect())
        }
        fn load(&self, id: &str) -> eyre::Result<Document> {
            self.docs
                .lock()
                .unwrap()
                .iter()
                .find(|d| d.id == id)
                .cloned()
                .ok_or_else(|| eyre::eyre!("no such doc"))
        }
        fn read(&self, id: &str, _section: Option<&str>) -> eyre::Result<String> {
            Ok(self.load(id)?.body)
        }
    }

    fn doc(source: &str, id: &str, body: &str, ver: u64) -> Document {
        Document {
            source: source.into(),
            id: id.into(),
            title: id.into(),
            body: body.into(),
            tags: vec![],
            links: vec![],
            headings: vec![],
            version: ver,
        }
    }

    #[test]
    fn sync_is_incremental_per_source() {
        let cache = tempfile::tempdir().unwrap();
        let kb = KbIndex::open(cache.path()).unwrap();

        let sources: Vec<Arc<dyn DocSource>> =
            vec![Arc::new(MockSource::new("m", vec![doc("m", "a", "alpha", 1)]))];

        let s = kb.sync(&sources).unwrap();
        assert_eq!(s.upserted, 1);

        // No changes -> nothing happens.
        let s = kb.sync(&sources).unwrap();
        assert_eq!(s, RefreshStats::default());
    }

    #[test]
    fn failed_list_skips_source_without_deleting() {
        let cache = tempfile::tempdir().unwrap();

        // First sync with a working source populates + persists the index.
        {
            let kb = KbIndex::open(cache.path()).unwrap();
            let sources: Vec<Arc<dyn DocSource>> =
                vec![Arc::new(MockSource::new("m", vec![doc("m", "a", "alpha", 1)]))];
            kb.sync(&sources).unwrap();
            assert_eq!(kb.reader.searcher().num_docs(), 1);
        }

        // Reopen the same cache dir; the source now fails list().
        let kb = KbIndex::open(cache.path()).unwrap();
        let failing = MockSource::new("m", vec![doc("m", "a", "alpha", 1)]);
        *failing.fail_list.lock().unwrap() = true;
        let sources: Vec<Arc<dyn DocSource>> = vec![Arc::new(failing)];

        let s = kb.sync(&sources).unwrap();
        assert_eq!(s.degraded, 1);
        assert_eq!(s.removed, 0, "a failing source must not delete its docs");
        assert_eq!(
            kb.reader.searcher().num_docs(),
            1,
            "index must stay intact when a source is unavailable"
        );
    }
}
