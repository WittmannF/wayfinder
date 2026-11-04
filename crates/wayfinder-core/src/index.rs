//! The derived index: SQLite with FTS5, rebuildable from the page files.
//!
//! Nothing here is authoritative. Deleting `index.db` costs one scan of the
//! pool; every row is derived from a markdown file.

use crate::error::{Error, Result};
use crate::page::{Kind, Page};
use crate::store::{StorePaths, resolve_root};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Schema version of the derived index; a mismatch rebuilds it.
const SCHEMA_VERSION: i64 = 1;

/// Filters one search may apply.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Keep pages of one kind.
    pub kind: Option<Kind>,
    /// Keep pages declaring one subject.
    pub subject: Option<String>,
    /// Maximum pages to return.
    pub limit: Option<usize>,
}

impl SearchFilters {
    /// Default page budget when the caller names none.
    const DEFAULT_LIMIT: usize = 8;

    /// The effective limit.
    fn limit(&self) -> usize {
        self.limit.unwrap_or(Self::DEFAULT_LIMIT).max(1)
    }
}

/// One ranked page from the index, before snippets are read from disk.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexHit {
    /// Page id.
    pub id: String,
    /// Relevance score; higher is better.
    pub score: f64,
}

/// What one sync changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Pages written or refreshed.
    pub indexed: usize,
    /// Pages whose files disappeared.
    pub removed: usize,
    /// Pages that were already current.
    pub unchanged: usize,
}

/// One page row as the index stores it.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    title: String,
    kind: Option<String>,
    updated: Option<String>,
    rev: String,
    body: String,
    stamp: i64,
}

/// An open handle to one pool's derived index.
pub struct Index {
    connection: Connection,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index").finish_non_exhaustive()
    }
}

impl Index {
    /// Open (and if needed create) the index file next to the pool.
    /// @param path - index file path
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| Error::Io(format!("create pool: {error}")))?;
        }
        let connection = Connection::open(path)?;
        let index = Self { connection };
        index.ensure_schema()?;
        Ok(index)
    }

    /// Open an index that lives only in memory, for tests and one-shot scans.
    pub fn open_memory() -> Result<Self> {
        let index = Self {
            connection: Connection::open_in_memory()?,
        };
        index.ensure_schema()?;
        Ok(index)
    }

    /// Create the tables and FTS index when they are absent or from an older
    /// schema.
    pub fn ensure_schema(&self) -> Result<()> {
        self.connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if version != SCHEMA_VERSION {
            self.connection.execute_batch(
                "DROP TABLE IF EXISTS pages; DROP TABLE IF EXISTS tags; DROP TABLE IF EXISTS subjects;
                 DROP TABLE IF EXISTS depends; DROP TABLE IF EXISTS edges;
                 DROP TABLE IF EXISTS pages_fts;",
            )?;
        }
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS pages(
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 kind TEXT,
                 updated TEXT,
                 rev TEXT NOT NULL,
                 body TEXT NOT NULL,
                 stamp INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tags(
                 page_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY(page_id, tag)
             );
             CREATE TABLE IF NOT EXISTS subjects(
                 page_id TEXT NOT NULL, subject TEXT NOT NULL, PRIMARY KEY(page_id, subject)
             );
             CREATE TABLE IF NOT EXISTS depends(
                 page_id TEXT NOT NULL, dependency TEXT NOT NULL, PRIMARY KEY(page_id, dependency)
             );
             CREATE TABLE IF NOT EXISTS edges(
                 src TEXT NOT NULL, dst TEXT NOT NULL, predicate TEXT NOT NULL,
                 PRIMARY KEY(src, dst, predicate)
             );
             CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst);",
        )?;
        match self.connection.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts
                 USING fts5(id UNINDEXED, title, body, tokenize = 'porter unicode61');",
        ) {
            Ok(()) => {}
            Err(error) => {
                return Err(Error::Index(format!(
                    "this SQLite build has no FTS5 support ({error}); rebuild with a bundled SQLite"
                )));
            }
        }
        self.connection
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        Ok(())
    }

    /// Bring the index in line with the pool directory, touching only pages
    /// whose file changed since the last sync.
    /// @param paths - the pool to scan
    pub fn sync(&self, paths: &StorePaths) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let entries = match fs::read_dir(&paths.memory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(stats);
            }
            Err(error) => return Err(Error::Io(format!("read pool: {error}"))),
        };
        let transaction = self.connection.unchecked_transaction()?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::Io(format!("read pool entry: {error}")))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            seen.insert(id.clone());
            let stamp = stamp_of(&path);
            let known: Option<String> = transaction
                .query_row(
                    "SELECT rev FROM pages WHERE id = ?1 AND stamp = ?2",
                    params![id, stamp],
                    |row| row.get(0),
                )
                .ok();
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let page = match Page::parse(&id, &text) {
                Ok(page) => page,
                Err(_) => continue,
            };
            if let Some(rev) = known
                && rev == page.rev()
            {
                stats.unchanged += 1;
                continue;
            }
            write_page(&transaction, &page, stamp)?;
            stats.indexed += 1;
        }
        let known: Vec<String> = {
            let mut statement = transaction.prepare("SELECT id FROM pages")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in known {
            if !seen.contains(&id) {
                remove_page_rows(&transaction, &id)?;
                stats.removed += 1;
            }
        }
        transaction.commit()?;
        Ok(stats)
    }

    /// Index one page that was just written, without rescanning the pool.
    /// @param paths - owning pool
    /// @param page - the page as it now stands
    pub fn sync_page(&self, paths: &StorePaths, page: &Page) -> Result<()> {
        let stamp = stamp_of(&paths.page(&page.id));
        write_page(&self.connection, page, stamp)
    }

    /// Drop one page from the index.
    /// @param id - page id
    pub fn remove_page(&self, id: &str) -> Result<()> {
        remove_page_rows(&self.connection, id)
    }

    /// Rank pages against a free-text query.
    /// @param query - what the caller asked; empty lists recent pages
    /// @param filters - kind, subject, and limit
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<IndexHit>> {
        let Some(expression) = fts_expression(query) else {
            return self.list(filters);
        };
        let mut sql = String::from(
            "SELECT pages.id, -bm25(pages_fts) AS score
               FROM pages_fts JOIN pages ON pages.id = pages_fts.id
              WHERE pages_fts MATCH ?1",
        );
        let mut values: Vec<rusqlite::types::Value> = vec![expression.into()];
        if let Some(kind) = filters.kind {
            sql.push_str(" AND pages.kind = ?");
            values.push(kind.as_str().to_owned().into());
        }
        if let Some(subject) = &filters.subject {
            sql.push_str(" AND pages.id IN (SELECT page_id FROM subjects WHERE subject = ?)");
            values.push(subject.clone().into());
        }
        sql.push_str(" ORDER BY score DESC, pages.id ASC LIMIT ?");
        values.push((filters.limit() as i64).into());
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            Ok(IndexHit {
                id: row.get(0)?,
                score: row.get::<_, f64>(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Pages in last-updated order, newest first.
    /// @param filters - kind, subject, and limit
    pub fn list(&self, filters: &SearchFilters) -> Result<Vec<IndexHit>> {
        let mut sql = String::from("SELECT id, 0.0 FROM pages WHERE 1 = 1");
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(kind) = filters.kind {
            sql.push_str(" AND kind = ?");
            values.push(kind.as_str().to_owned().into());
        }
        if let Some(subject) = &filters.subject {
            sql.push_str(" AND id IN (SELECT page_id FROM subjects WHERE subject = ?)");
            values.push(subject.clone().into());
        }
        sql.push_str(" ORDER BY COALESCE(updated, '') DESC, id ASC LIMIT ?");
        values.push((filters.limit() as i64).into());
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            Ok(IndexHit {
                id: row.get(0)?,
                score: row.get::<_, f64>(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every edge the pool declares, as `(from, to, predicate)`.
    pub fn edges(&self) -> Result<Vec<(String, String, String)>> {
        let mut statement = self
            .connection
            .prepare("SELECT src, dst, predicate FROM edges")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Pages one edge away from any of `seeds`, with the predicate that reached
    /// them, excluding the seeds themselves.
    /// @param seeds - page ids the caller already matched
    pub fn neighbours(&self, seeds: &[String]) -> Result<Vec<(String, String)>> {
        let mut found: Vec<(String, String)> = Vec::new();
        if seeds.is_empty() {
            return Ok(found);
        }
        let placeholders = std::iter::repeat_n("?", seeds.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT dst, predicate FROM edges WHERE src IN ({placeholders})
             UNION SELECT src, predicate FROM edges WHERE dst IN ({placeholders})"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let values: Vec<rusqlite::types::Value> = seeds
            .iter()
            .chain(seeds.iter())
            .cloned()
            .map(rusqlite::types::Value::from)
            .collect();
        let rows = statement.query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            found.push(row?);
        }
        Ok(found)
    }

    /// Count of indexed pages.
    pub fn page_count(&self) -> Result<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM pages", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Revision the index recorded for one page, when it is indexed.
    /// @param id - page id
    pub fn rev_of(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT rev FROM pages WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .ok())
    }

    /// The pool a caller asked for, resolved the way `Store::open` does.
    /// @param start - directory to resolve from
    pub fn pool_paths(start: &Path) -> StorePaths {
        StorePaths::under(resolve_root(start))
    }
}

/// Write one page's rows and its FTS entry.
fn write_page(connection: &Connection, page: &Page, stamp: i64) -> Result<()> {
    remove_page_rows(connection, &page.id)?;
    let row = Row {
        id: page.id.clone(),
        title: page.front.title.clone(),
        kind: page.front.kind.map(|kind| kind.as_str().to_owned()),
        updated: page.front.updated.clone(),
        rev: page.rev(),
        body: page.body.clone(),
        stamp,
    };
    connection.execute(
        "INSERT INTO pages(id, title, kind, updated, rev, body, stamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![row.id, row.title, row.kind, row.updated, row.rev, row.body, row.stamp],
    )?;
    connection.execute(
        "INSERT INTO pages_fts(id, title, body) VALUES (?1, ?2, ?3)",
        params![row.id, row.title, row.body],
    )?;
    for tag in &page.front.tags {
        connection.execute(
            "INSERT OR IGNORE INTO tags(page_id, tag) VALUES (?1, ?2)",
            params![row.id, tag],
        )?;
    }
    for subject in &page.front.subjects {
        connection.execute(
            "INSERT OR IGNORE INTO subjects(page_id, subject) VALUES (?1, ?2)",
            params![row.id, subject],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO edges(src, dst, predicate) VALUES (?1, ?2, 'about')",
            params![row.id, subject],
        )?;
    }
    for dependency in &page.front.depends {
        connection.execute(
            "INSERT OR IGNORE INTO depends(page_id, dependency) VALUES (?1, ?2)",
            params![row.id, dependency],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO edges(src, dst, predicate) VALUES (?1, ?2, 'depends')",
            params![row.id, format!("package:{dependency}")],
        )?;
    }
    if let Some(supersedes) = &page.front.supersedes {
        connection.execute(
            "INSERT OR IGNORE INTO edges(src, dst, predicate) VALUES (?1, ?2, 'supersedes')",
            params![row.id, supersedes],
        )?;
    }
    for anchor in &page.front.anchors {
        connection.execute(
            "INSERT OR IGNORE INTO edges(src, dst, predicate) VALUES (?1, ?2, 'cites')",
            params![row.id, format!("path:{}", anchor.path)],
        )?;
    }
    Ok(())
}

/// Drop a page and everything derived from it.
fn remove_page_rows(connection: &Connection, id: &str) -> Result<()> {
    connection.execute("DELETE FROM pages WHERE id = ?1", params![id])?;
    connection.execute("DELETE FROM pages_fts WHERE id = ?1", params![id])?;
    connection.execute("DELETE FROM tags WHERE page_id = ?1", params![id])?;
    connection.execute("DELETE FROM subjects WHERE page_id = ?1", params![id])?;
    connection.execute("DELETE FROM depends WHERE page_id = ?1", params![id])?;
    connection.execute("DELETE FROM edges WHERE src = ?1", params![id])?;
    Ok(())
}

/// Change stamp of one file: mtime in milliseconds, or `0` when unavailable.
fn stamp_of(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as i64)
        .unwrap_or(0)
}

/// Build an FTS5 MATCH expression from free text: every word, all required.
/// Returns `None` for a query with no usable words, which the caller answers
/// with the recency list instead.
fn fts_expression(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in query.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms.retain(|term| term.len() > 1);
    terms.dedup();
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .into_iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}
