//! evoclaw-kb-core — a source-agnostic BM25 knowledge-base engine.
//!
//! A [`KbIndex`] holds a single tantivy index over documents pulled from any
//! number of [`DocSource`]s (Obsidian vault, Feishu wiki, ...). Each document
//! carries a `source` tag so one unified `kb_search` ranks across every
//! source at once, and results say which source they came from.

pub mod document;
pub mod error;
pub mod index;
pub mod search;

pub use document::{DocSource, Document};
pub use error::{KbError, Result};
pub use index::{KbIndex, RefreshStats};
pub use search::SearchHit;
