# evoclaw-kb-core

Source-agnostic **BM25 knowledge-base engine** for the EvoClaw ecosystem. This
is a library crate — you don't install or run it directly; it's a dependency of
the knowledge-base MCP server
[`evoclaw-mcp-kb`](https://github.com/DevEloLin/evoclaw-mcp-kb).

> New here? Read the solution overview:
> [`evoclaw-mcp-kb/OVERVIEW.md`](https://github.com/DevEloLin/evoclaw-mcp-kb/blob/main/OVERVIEW.md).

## What it provides

- A generic `Document` model and a `DocSource` trait — any source (Obsidian
  vault, Feishu wiki, ...) implements `list` / `load` / `read`.
- `KbIndex` — a single [tantivy](https://github.com/quickwit-oss/tantivy) BM25
  index over all sources, with jieba Chinese word segmentation, title/heading
  boosts, snippets, and tag / source / path-prefix filters.
- Per-source incremental refresh: a single `uid` delete key, and a source whose
  `list()` fails is **skipped** (its documents are never wrongly deleted).

## Use

```toml
[dependencies]
evoclaw-kb-core = { git = "https://github.com/DevEloLin/evoclaw-kb-core" }
```

```rust
use evoclaw_kb_core::{KbIndex, DocSource};

let kb = KbIndex::open(&cache_dir)?;
kb.sync(&sources)?;                          // sources: &[Arc<dyn DocSource>]
let hits = kb.search("query", 8, None, None, None)?;
```

## Development

```bash
cargo test
```

## License

MIT
