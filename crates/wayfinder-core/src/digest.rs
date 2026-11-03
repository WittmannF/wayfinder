//! The prompt digest: what a harness injects so a session knows the pool exists
//! before it decides to look.

use crate::error::Result;
use crate::store::Store;

/// How much the digest may spend.
#[derive(Debug, Clone, Copy)]
pub struct DigestOptions {
    /// Maximum pages to list.
    pub pages: usize,
    /// Maximum pages once four bytes per token are counted.
    pub tokens: usize,
}

impl Default for DigestOptions {
    fn default() -> Self {
        Self {
            pages: 8,
            tokens: 500,
        }
    }
}

/// Render the digest block for one pool. An empty pool yields an empty string,
/// so a harness can inject it unconditionally.
/// @param store - the pool to describe
/// @param options - page and token budget
pub fn digest(store: &Store, options: DigestOptions) -> Result<String> {
    let index = store.index()?;
    index.sync(store.paths())?;
    let total = index.page_count()?;
    if total == 0 {
        return Ok(String::new());
    }
    let budget = (options.tokens.saturating_mul(4)).max(200);
    let listed = index.list(&crate::index::SearchFilters {
        kind: None,
        subject: None,
        limit: Some(options.pages.max(1)),
    })?;
    let mut text = format!(
        "## Workspace memory (wayfinder)\nThis workspace has {total} memory page{} written by earlier sessions.\n\
         Search before you re-derive: `wayfinder search \"<question>\"`, then `wayfinder read <id>`.\n",
        if total == 1 { "" } else { "s" },
    );
    let mut spent = text.len();
    let mut written = 0usize;
    for hit in listed {
        let Ok(page) = store.read(&hit.id) else {
            continue;
        };
        let stamp = page
            .front
            .updated
            .as_deref()
            .map(|updated| updated.get(..10).unwrap_or(updated))
            .unwrap_or("unknown");
        let line = format!(
            "- {} (id: {}, updated {stamp})\n",
            page.front.title, page.id
        );
        if spent + line.len() > budget {
            text.push_str(&format!(
                "- …{} more page{} not listed; search to find them.\n",
                total - written,
                if total - written == 1 { "" } else { "s" },
            ));
            break;
        }
        spent += line.len();
        written += 1;
        text.push_str(&line);
    }
    text.push_str("Pages that cite a file carry a fingerprint; `wayfinder check <id>` says whether it still matches.\n");
    Ok(text)
}
