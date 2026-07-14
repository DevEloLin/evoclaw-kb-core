# evoclaw-kb-core

[English](README.md) · 中文

面向 EvoClaw 生态的**数据源无关 BM25 知识库引擎**。这是一个**库** crate —— 你不直接安装或运行它，它是知识库 MCP server [`evoclaw-mcp-kb`](https://github.com/DevEloLin/evoclaw-mcp-kb) 的依赖。

> 第一次用？先看总览：[`evoclaw-mcp-kb/OVERVIEW.md`](https://github.com/DevEloLin/evoclaw-mcp-kb/blob/main/OVERVIEW.md)。

## 它提供什么

- 泛化的 `Document` 模型 + `DocSource` trait —— 任何来源（Obsidian vault、飞书 wiki…）实现 `list` / `load` / `read` 即可接入。
- `KbIndex` —— 覆盖所有来源的**单一** [tantivy](https://github.com/quickwit-oss/tantivy) BM25 索引，含 jieba 中文分词、title/heading 加权、snippet，以及 tag / source / path-prefix 过滤。
- 按来源增量刷新：单一 `uid` 删除键；某来源 `list()` 失败会被**跳过**（它的文档绝不会被误删）。

## 用法

```toml
[dependencies]
evoclaw-kb-core = { git = "https://github.com/DevEloLin/evoclaw-kb-core" }
```

```rust
use evoclaw_kb_core::{KbIndex, DocSource};

let kb = KbIndex::open(&cache_dir)?;
kb.sync(&sources)?;                          // sources: &[Arc<dyn DocSource>]
let hits = kb.search("查询", 8, None, None, None)?;
```

## 开发

```bash
cargo test
```

## 许可

MIT
