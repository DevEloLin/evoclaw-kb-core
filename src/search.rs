//! BM25 search across all sources: field-boosted query parsing, optional
//! tag / source / path-prefix filters, and snippet generation.

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{TantivyDocument, Term};

use crate::error::Result;
use crate::index::KbIndex;

const TITLE_BOOST: f32 = 2.0;
const HEADINGS_BOOST: f32 = 1.5;
const SNIPPET_MAX_CHARS: usize = 200;
const PREFIX_OVERFETCH: usize = 5;
const PREFIX_OVERFETCH_CAP: usize = 200;

/// One search result. `source` + `id` together identify the note for a
/// follow-up `kb_read`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub source: String,
    pub id: String,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
}

impl KbIndex {
    /// Run a BM25 query across all indexed sources.
    /// - `tags`: keep notes carrying ANY listed tag.
    /// - `source`: restrict to one source id.
    /// - `path_prefix`: keep notes whose source-local id starts with it.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        tags: Option<&[String]>,
        source: Option<&str>,
        path_prefix: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        let searcher = self.reader.searcher();
        let f = &self.fields;
        let limit = limit.max(1);

        let mut parser =
            QueryParser::for_index(&self.index, vec![f.title, f.body, f.headings]);
        parser.set_field_boost(f.title, TITLE_BOOST);
        parser.set_field_boost(f.headings, HEADINGS_BOOST);
        let (text_query, _errs) = parser.parse_query_lenient(query);

        let tags = tags.filter(|t| !t.is_empty());
        let final_query: Box<dyn Query> = if tags.is_some() || source.is_some() {
            let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, text_query)];
            if let Some(ts) = tags {
                let any: Vec<(Occur, Box<dyn Query>)> = ts
                    .iter()
                    .map(|t| {
                        let q: Box<dyn Query> = Box::new(TermQuery::new(
                            Term::from_field_text(f.tags, t),
                            IndexRecordOption::Basic,
                        ));
                        (Occur::Should, q)
                    })
                    .collect();
                clauses.push((Occur::Must, Box::new(BooleanQuery::new(any))));
            }
            if let Some(src) = source {
                clauses.push((
                    Occur::Must,
                    Box::new(TermQuery::new(
                        Term::from_field_text(f.source, src),
                        IndexRecordOption::Basic,
                    )),
                ));
            }
            Box::new(BooleanQuery::new(clauses))
        } else {
            text_query
        };

        let fetch = if path_prefix.is_some() {
            limit
                .saturating_mul(PREFIX_OVERFETCH)
                .min(PREFIX_OVERFETCH_CAP)
                .max(limit)
        } else {
            limit
        };
        let collector = TopDocs::with_limit(fetch).order_by_score();
        let top = searcher.search(&*final_query, &collector)?;

        let mut snippet_gen = SnippetGenerator::create(&searcher, &*final_query, f.body)?;
        snippet_gen.set_max_num_chars(SNIPPET_MAX_CHARS);

        let mut hits = Vec::with_capacity(limit);
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            let id = str_field(&doc, f.id);
            if let Some(prefix) = path_prefix {
                if !id.starts_with(prefix) {
                    continue;
                }
            }
            let body = str_field(&doc, f.body);
            let snippet = snippet_gen.snippet(&body);
            let snippet_text = if snippet.fragment().is_empty() {
                body.chars().take(SNIPPET_MAX_CHARS).collect()
            } else {
                snippet.fragment().to_string()
            };

            hits.push(SearchHit {
                source: str_field(&doc, f.source),
                id,
                title: str_field(&doc, f.title),
                score,
                snippet: snippet_text,
                tags: doc
                    .get_all(f.tags)
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }
}

fn str_field(doc: &TantivyDocument, field: tantivy::schema::Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use crate::document::{DocSource, Document};
    use crate::index::KbIndex;
    use std::sync::{Arc, Mutex};

    struct VecSource {
        id: String,
        docs: Mutex<Vec<Document>>,
    }
    impl DocSource for VecSource {
        fn source_id(&self) -> &str {
            &self.id
        }
        fn list(&self) -> eyre::Result<Vec<(String, u64)>> {
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
                .ok_or_else(|| eyre::eyre!("missing"))
        }
        fn read(&self, id: &str, _s: Option<&str>) -> eyre::Result<String> {
            Ok(self.load(id)?.body)
        }
    }

    fn d(source: &str, id: &str, title: &str, body: &str, tags: &[&str]) -> Document {
        Document {
            source: source.into(),
            id: id.into(),
            title: title.into(),
            body: body.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            links: vec![],
            headings: vec![title.into()],
            version: 1,
        }
    }

    fn indexed(docs: Vec<(&str, Vec<Document>)>) -> (tempfile::TempDir, KbIndex) {
        let cache = tempfile::tempdir().unwrap();
        let kb = KbIndex::open(cache.path()).unwrap();
        let sources: Vec<Arc<dyn DocSource>> = docs
            .into_iter()
            .map(|(sid, ds)| {
                Arc::new(VecSource {
                    id: sid.into(),
                    docs: Mutex::new(ds),
                }) as Arc<dyn DocSource>
            })
            .collect();
        kb.sync(&sources).unwrap();
        (cache, kb)
    }

    #[test]
    fn ranks_across_sources_and_reports_source() {
        let (_c, kb) = indexed(vec![
            ("obsidian", vec![d("obsidian", "a.md", "Apple", "sweet red fruit", &[])]),
            ("feishu", vec![d("feishu", "tok1", "Car", "a road vehicle", &[])]),
        ]);
        let hits = kb.search("sweet fruit", 5, None, None, None).unwrap();
        assert_eq!(hits[0].source, "obsidian");
        assert_eq!(hits[0].id, "a.md");
        assert!(hits[0].snippet.to_lowercase().contains("fruit"));
    }

    #[test]
    fn source_filter_restricts_results() {
        let (_c, kb) = indexed(vec![
            ("obsidian", vec![d("obsidian", "a.md", "Note", "shared keyword", &[])]),
            ("feishu", vec![d("feishu", "tok1", "Doc", "shared keyword", &[])]),
        ]);
        let hits = kb.search("shared keyword", 5, None, Some("feishu"), None).unwrap();
        let srcs: Vec<_> = hits.iter().map(|h| h.source.as_str()).collect();
        assert_eq!(srcs, vec!["feishu"]);
    }

    #[test]
    fn tag_filter_any() {
        let (_c, kb) = indexed(vec![(
            "s",
            vec![
                d("s", "p", "P", "project notes", &["work"]),
                d("s", "h", "H", "project notes", &["home"]),
            ],
        )]);
        let hits = kb
            .search("project", 5, Some(&["work".to_string()]), None, None)
            .unwrap();
        let ids: Vec<_> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["p"]);
    }

    #[test]
    fn chinese_word_segmentation() {
        let (_c, kb) = indexed(vec![(
            "s",
            vec![
                d("s", "cook", "番茄炒蛋", "先炒蛋再放番茄，加糖提鲜。", &["cooking"]),
                d("s", "k8s", "部署", "用 kubectl 滚动更新服务。", &[]),
            ],
        )]);
        let hits = kb.search("提鲜", 5, None, None, None).unwrap();
        assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["cook"]);
    }

    #[test]
    fn title_outranks_body() {
        let (_c, kb) = indexed(vec![(
            "s",
            vec![
                d("s", "d1", "Banana", "unrelated body text", &[]),
                d("s", "d2", "Unrelated", "mentions banana once", &[]),
            ],
        )]);
        let hits = kb.search("banana", 5, None, None, None).unwrap();
        assert_eq!(hits[0].id, "d1");
    }
}
