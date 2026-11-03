//! The pool: markdown files on disk, an atomic writer, and a rebuildable index.

use crate::anchors::anchor_spec;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::page::{Anchor, Kind, Page, merge_tags, normalize_subject, slugify};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Marker directory that makes a directory the pool's project root.
const POOL_DIR: &str = ".wayfinder";
/// Pages live here, one file per page.
const MEMORY_DIR: &str = "memory";
/// Derived SQLite index, safe to delete.
const INDEX_FILE: &str = "index.db";
/// Cross-process write lock.
const LOCK_FILE: &str = "write.lock";

/// Where a pool's parts live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    /// Project root the pool belongs to.
    pub root: PathBuf,
    /// `<root>/.wayfinder`
    pub pool: PathBuf,
    /// `<root>/.wayfinder/memory`
    pub memory: PathBuf,
    /// `<root>/.wayfinder/index.db`
    pub index: PathBuf,
    /// `<root>/.wayfinder/write.lock`
    pub lock: PathBuf,
}

impl StorePaths {
    /// Paths of the pool under one project root.
    /// @param root - the project root
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let pool = root.join(POOL_DIR);
        Self {
            memory: pool.join(MEMORY_DIR),
            index: pool.join(INDEX_FILE),
            lock: pool.join(LOCK_FILE),
            pool,
            root,
        }
    }

    /// Path of one page file.
    /// @param id - page id
    pub fn page(&self, id: &str) -> PathBuf {
        self.memory.join(format!("{id}.md"))
    }
}

/// One anchor a caller asked for.
pub type AnchorInput = Anchor;

/// What one save writes.
#[derive(Debug, Clone, Default)]
pub struct SaveRequest {
    /// Page id; derived from the title when absent.
    pub id: Option<String>,
    /// Title written to frontmatter.
    pub title: String,
    /// Body written under the frontmatter.
    pub body: String,
    /// Tags to merge into the page's existing ones.
    pub tags: Vec<String>,
    /// What the page is for.
    pub kind: Option<Kind>,
    /// Subjects to declare.
    pub subjects: Vec<String>,
    /// Packages the page depends on.
    pub depends: Vec<String>,
    /// Page this one replaces.
    pub supersedes: Option<String>,
    /// Where the page came from.
    pub source: Option<String>,
    /// Files the page cites, already resolved against the root.
    pub anchors: Vec<Anchor>,
    /// Revision the writer saw; a different current revision is refused.
    pub expected_rev: Option<String>,
    /// Overwrite a file whose frontmatter cannot be parsed.
    pub force: bool,
}

impl SaveRequest {
    /// A save with a title and body and nothing else declared.
    /// @param title - page title
    /// @param body - markdown body
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            ..Self::default()
        }
    }
}

/// What one save produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveResult {
    /// Page id written.
    pub id: String,
    /// Revision after the write.
    pub rev: String,
    /// Whether the page is new to the pool.
    pub created: bool,
    /// Anchors whose file could not be read, so no digest was recorded.
    pub unresolved_anchors: Vec<String>,
}

/// A pool rooted at one project directory.
#[derive(Debug, Clone)]
pub struct Store {
    paths: StorePaths,
}

impl Store {
    /// Open the pool that serves `start`, resolving the project root the way a
    /// repository does: `--root`, then `WAYFINDER_ROOT`, then the nearest
    /// ancestor holding `.wayfinder/` or `.git/`, then `start` itself.
    /// @param start - directory to resolve from
    pub fn open(start: impl AsRef<Path>) -> Result<Self> {
        let root = resolve_root(start.as_ref());
        let store = Self {
            paths: StorePaths::under(root),
        };
        if !store.paths.memory.is_dir() {
            return Err(Error::NoPool(store.paths.root.display().to_string()));
        }
        Ok(store)
    }

    /// Create the pool under `root` when it is absent.
    /// @param root - project root to initialise
    /// @returns the open store
    pub fn init(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| root.as_ref().to_path_buf());
        let paths = StorePaths::under(root);
        fs::create_dir_all(&paths.memory)
            .map_err(|error| Error::Io(format!("create pool: {error}")))?;
        let store = Self { paths };
        store.index()?.ensure_schema()?;
        Ok(store)
    }

    /// Where this pool's parts live.
    pub fn paths(&self) -> &StorePaths {
        &self.paths
    }

    /// Project root of this pool.
    pub fn root(&self) -> &Path {
        &self.paths.root
    }

    /// Open the derived index of this pool.
    pub fn index(&self) -> Result<Index> {
        Index::open(&self.paths.index)
    }

    /// Every page in the pool, in id order, read from disk.
    pub fn pages(&self) -> Result<Vec<Page>> {
        let mut pages = Vec::new();
        let entries = fs::read_dir(&self.paths.memory)
            .map_err(|error| Error::Io(format!("read pool: {error}")))?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::Io(format!("read pool entry: {error}")))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let text = fs::read_to_string(&path)
                .map_err(|error| Error::Io(format!("read {}: {error}", path.display())))?;
            let page = Page::parse(id, &text)?;
            pages.push(page);
        }
        pages.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(pages)
    }

    /// Read one page.
    /// @param id - page id
    pub fn read(&self, id: &str) -> Result<Page> {
        let path = self.paths.page(id);
        let text = fs::read_to_string(&path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::UnknownPage(id.to_owned()),
            _ => Error::Io(format!("read {id}: {error}")),
        })?;
        Page::parse(id, &text)
    }

    /// Save one page: merge into what is there, refuse a stale fence, write
    /// atomically, and refresh the index entry.
    /// @param request - what to write
    pub fn save(&self, request: &SaveRequest) -> Result<SaveResult> {
        let title = request.title.trim();
        if title.is_empty() {
            return Err(Error::Invalid("a page needs a title".to_owned()));
        }
        let id = match request.id.as_deref() {
            Some(raw) if !raw.trim().is_empty() => slugify(raw),
            _ => slugify(title),
        };
        let lock = self.lock()?;
        let result = self.save_locked(&id, request, title);
        drop(lock);
        result
    }

    /// The write body, run while the pool lock is held.
    fn save_locked(&self, id: &str, request: &SaveRequest, title: &str) -> Result<SaveResult> {
        let existing = match self.read(id) {
            Ok(page) => Some(page),
            Err(Error::UnknownPage(_)) => None,
            Err(error @ Error::Frontmatter(_)) if !request.force => {
                return Err(Error::Frontmatter(format!(
                    "{id}: {error}; pass --force to overwrite the file"
                )));
            }
            Err(Error::Frontmatter(_)) => None,
            Err(other) => return Err(other),
        };
        if let (Some(expected), Some(page)) = (request.expected_rev.as_deref(), existing.as_ref()) {
            let current = page.rev();
            if current != expected {
                return Err(Error::StaleRevision {
                    id: id.to_owned(),
                    expected: expected.to_owned(),
                    current,
                });
            }
        }
        let now = now_rfc3339();
        let mut unresolved_anchors = Vec::new();
        let anchors: Vec<Anchor> = request
            .anchors
            .iter()
            .map(|anchor| {
                let resolved = anchor_spec(&self.paths.root, &anchor.spec());
                if resolved.sha.is_none() {
                    unresolved_anchors.push(resolved.spec());
                }
                // A caller that already named a digest keeps it.
                if anchor.sha.is_some() {
                    anchor.clone()
                } else {
                    resolved
                }
            })
            .collect();

        let mut front = existing
            .as_ref()
            .map(|page| page.front.clone())
            .unwrap_or_default();
        front.title = title.to_owned();
        front.tags = merge_tags(&front.tags, &request.tags);
        if let Some(kind) = request.kind {
            front.kind = Some(kind);
        }
        for subject in &request.subjects {
            let normalized = normalize_subject(subject);
            if !normalized.is_empty() && !front.subjects.contains(&normalized) {
                front.subjects.push(normalized);
            }
        }
        for dependency in &request.depends {
            let dependency = dependency.trim();
            if !dependency.is_empty() && !front.depends.iter().any(|kept| kept == dependency) {
                front.depends.push(dependency.to_owned());
            }
        }
        if request.supersedes.is_some() {
            front.supersedes = request.supersedes.as_deref().map(slugify);
        }
        if request.source.is_some() {
            front.source = request.source.clone();
        }
        front.anchors = anchors;
        front.created.get_or_insert_with(|| now.clone());
        front.updated = Some(now);

        let body = if request.body.trim().is_empty() {
            existing
                .as_ref()
                .map(|page| page.body.clone())
                .unwrap_or_default()
        } else {
            request.body.clone()
        };
        let page = Page {
            id: id.to_owned(),
            front,
            body,
        };
        let bytes = page.render();
        self.write_atomic(&self.paths.page(id), &bytes)?;
        self.index()?.sync_page(&self.paths, &page)?;
        Ok(SaveResult {
            id: id.to_owned(),
            rev: page.rev(),
            created: existing.is_none(),
            unresolved_anchors,
        })
    }

    /// Remove one page and its index entry.
    /// @param id - page id
    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.paths.page(id);
        let lock = self.lock()?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::UnknownPage(id.to_owned()));
            }
            Err(error) => return Err(Error::Io(format!("delete {id}: {error}"))),
        }
        drop(lock);
        self.index()?.remove_page(id)?;
        Ok(())
    }

    /// Rebuild the derived index from the page files.
    pub fn reindex(&self) -> Result<crate::index::SyncStats> {
        let lock = self.lock()?;
        let stats = self.index()?.sync(&self.paths)?;
        drop(lock);
        Ok(stats)
    }

    /// Take the pool's exclusive write lock, so concurrent writers serialize
    /// their index updates while the page file itself stays atomic.
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.paths.pool)
            .map_err(|error| Error::Io(format!("create pool: {error}")))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.paths.lock)
            .map_err(|error| Error::Io(format!("open write lock: {error}")))?;
        file.lock_exclusive()
            .map_err(|error| Error::Io(format!("lock pool: {error}")))?;
        Ok(file)
    }

    /// Write bytes through a temporary file in the same directory, then rename:
    /// a reader never sees a half-written page.
    fn write_atomic(&self, target: &Path, bytes: &str) -> Result<()> {
        let directory = target.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(directory)
            .map_err(|error| Error::Io(format!("create pool: {error}")))?;
        let temporary = directory.join(format!(
            ".{}.{}.tmp",
            target
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("page"),
            std::process::id()
        ));
        {
            let mut file = File::create(&temporary)
                .map_err(|error| Error::Io(format!("write {}: {error}", temporary.display())))?;
            file.write_all(bytes.as_bytes())
                .map_err(|error| Error::Io(format!("write page: {error}")))?;
            file.sync_all()
                .map_err(|error| Error::Io(format!("flush page: {error}")))?;
        }
        fs::rename(&temporary, target).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            Error::Io(format!("commit {}: {error}", target.display()))
        })
    }
}

/// Resolve the project root that serves `start`.
/// @param start - directory to resolve from
pub fn resolve_root(start: &Path) -> PathBuf {
    if let Ok(env_root) = std::env::var("WAYFINDER_ROOT") {
        let candidate = PathBuf::from(env_root);
        if !candidate.as_os_str().is_empty() {
            return candidate;
        }
    }
    let absolute = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for ancestor in absolute.ancestors() {
        if ancestor.join(POOL_DIR).is_dir() {
            return ancestor.to_path_buf();
        }
    }
    for ancestor in absolute.ancestors() {
        if ancestor.join(".git").exists() {
            return ancestor.to_path_buf();
        }
    }
    absolute
}

/// Resolve the project root from the current directory.
pub fn default_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_root(&cwd)
}

/// Current time as RFC 3339, in UTC.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
