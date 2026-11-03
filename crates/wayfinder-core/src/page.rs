//! The page: frontmatter an agent declares, and the body a later session reads.
//!
//! The frontmatter is a closed subset of YAML — scalars, inline or block string
//! lists, and a block list of anchor maps — parsed here rather than with a
//! general YAML crate so an unknown key is reported instead of silently kept.

use crate::error::{Error, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// What a page is for. `pointer` ranks below the others: it names a place, not
/// a conclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A verified statement about the workspace.
    Fact,
    /// A choice, with the alternatives that lost.
    Decision,
    /// A trap that cost a session once.
    Gotcha,
    /// A name and where to look, without the answer.
    Pointer,
}

impl Kind {
    /// Wire and file spelling of this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Gotcha => "gotcha",
            Self::Pointer => "pointer",
        }
    }

    /// Parse a frontmatter `kind` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "fact" => Some(Self::Fact),
            "decision" => Some(Self::Decision),
            "gotcha" => Some(Self::Gotcha),
            "pointer" => Some(Self::Pointer),
            _ => None,
        }
    }

    /// Whether this kind carries an answer rather than a pointer to one.
    pub fn is_substantive(self) -> bool {
        self != Self::Pointer
    }
}

/// One cited file, with the hash the file had when the page was written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Anchor {
    /// Repository-relative path the page cites.
    pub path: String,
    /// One-based line the citation starts at, when the author named one.
    pub line: Option<u32>,
    /// Hex digest of the file at save time; absent for a hand-written anchor.
    pub sha: Option<String>,
}

impl Anchor {
    /// Parse `path`, `path:line`, or `path:line@sha` into one anchor.
    pub fn parse(spec: &str) -> Self {
        let spec = spec.trim();
        let (rest, sha) = match spec.rsplit_once('@') {
            Some((rest, digest))
                if digest.len() == 16 && digest.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                (rest, Some(digest.to_ascii_lowercase()))
            }
            _ => (spec, None),
        };
        match rest.rsplit_once(':') {
            Some((path, line)) if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => {
                Self {
                    path: path.to_owned(),
                    line: line.parse().ok(),
                    sha,
                }
            }
            _ => Self {
                path: rest.to_owned(),
                line: None,
                sha,
            },
        }
    }

    /// How citations render: `path` or `path:line`.
    pub fn spec(&self) -> String {
        match self.line {
            Some(line) => format!("{}:{line}", self.path),
            None => self.path.clone(),
        }
    }
}

/// The declared half of a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    /// Human title; the page id comes from the file name.
    pub title: String,
    /// Free-form labels, deduplicated in declaration order.
    pub tags: Vec<String>,
    /// What the page is for.
    pub kind: Option<Kind>,
    /// Subjects the page is about, normalized to `namespace:key`.
    pub subjects: Vec<String>,
    /// Packages or components this page depends on.
    pub depends: Vec<String>,
    /// Page id this page replaces.
    pub supersedes: Option<String>,
    /// RFC 3339 creation stamp.
    pub created: Option<String>,
    /// RFC 3339 last-write stamp.
    pub updated: Option<String>,
    /// Where the page came from (`session:<id>`, `run:<id>`).
    pub source: Option<String>,
    /// Cited files.
    pub anchors: Vec<Anchor>,
}

/// One page: its id, its declared half, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Stable id, taken from the file name without `.md`.
    pub id: String,
    /// Declared fields.
    pub front: Frontmatter,
    /// Markdown body.
    pub body: String,
}

impl Page {
    /// Revision of this page's bytes: a stale writer compares against it.
    pub fn rev(&self) -> String {
        rev_of(&self.front, &self.body)
    }

    /// Render the page to the bytes that are written to disk.
    pub fn render(&self) -> String {
        render(&self.front, &self.body)
    }

    /// Parse one page file's contents.
    pub fn parse(id: &str, text: &str) -> Result<Self> {
        let (front, body) = parse_frontmatter(text)?;
        Ok(Self {
            id: id.to_owned(),
            front,
            body,
        })
    }
}

/// Parse frontmatter and body out of one file's text.
pub fn parse_frontmatter(text: &str) -> Result<(Frontmatter, String)> {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return Ok((Frontmatter::default(), String::new()));
    };
    if first.trim() != "---" {
        // A page without frontmatter is a plain note: keep the body, declare nothing.
        return Ok((
            Frontmatter::default(),
            text.trim_start_matches('\n').to_owned(),
        ));
    }
    let mut front = Frontmatter::default();
    let mut body = String::new();
    let mut closed = false;
    let mut block_key: Option<String> = None;
    let mut anchor: Option<Anchor> = None;
    for line in lines.by_ref() {
        if !closed && line.trim() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("  - ")
            .or_else(|| line.strip_prefix("- "))
        {
            match block_key.as_deref() {
                Some("anchors") => {
                    if let Some(done) = anchor.take() {
                        front.anchors.push(done);
                    }
                    anchor = Some(Anchor::parse(strip_quotes(rest.trim())));
                }
                Some("tags") => push_unique(&mut front.tags, strip_quotes(rest.trim())),
                Some("subject") => {
                    push_unique(&mut front.subjects, &normalize_subject(rest.trim()))
                }
                Some("depends") => push_unique(&mut front.depends, strip_quotes(rest.trim())),
                _ => {
                    return Err(Error::Frontmatter(format!(
                        "block list item under no key: {line:?}"
                    )));
                }
            }
            continue;
        }
        // Fields of one anchor are indented under its `- path` item; a new
        // top-level key closes it.
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            if let Some(current) = anchor.as_mut() {
                if let Some((key, value)) = line.trim().split_once(':') {
                    let value = strip_quotes(value.trim());
                    match key.trim() {
                        "path" => current.path = value.to_owned(),
                        "line" => current.line = value.parse().ok(),
                        "sha" => current.sha = Some(value.to_ascii_lowercase()),
                        _ => {}
                    }
                    continue;
                }
            }
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(Error::Frontmatter(format!(
                "expected `key: value`, found {line:?}"
            )));
        };
        if let Some(done) = anchor.take() {
            front.anchors.push(done);
        }
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "title" => front.title = strip_quotes(value).to_owned(),
            "tags" => {
                block_key = Some("tags".to_owned());
                extend_inline(&mut front.tags, value, |entry| {
                    strip_quotes(entry).to_owned()
                });
            }
            "kind" => {
                block_key = None;
                let raw = strip_quotes(value);
                front.kind = Kind::parse(raw);
                if front.kind.is_none() && !raw.is_empty() {
                    return Err(Error::Frontmatter(format!(
                        "unknown kind {raw:?}; use fact, decision, gotcha, or pointer"
                    )));
                }
            }
            "subject" | "subjects" => {
                block_key = Some("subject".to_owned());
                extend_inline(&mut front.subjects, value, |s| normalize_subject(s));
            }
            "depends" => {
                block_key = Some("depends".to_owned());
                extend_inline(&mut front.depends, value, |entry| {
                    strip_quotes(entry).to_owned()
                });
            }
            "supersedes" => {
                block_key = None;
                let raw = strip_quotes(value);
                front.supersedes = (!raw.is_empty()).then(|| raw.to_owned());
            }
            "created" => front.created = non_empty(strip_quotes(value)),
            "updated" => front.updated = non_empty(strip_quotes(value)),
            "source" => front.source = non_empty(strip_quotes(value)),
            "anchors" => {
                block_key = Some("anchors".to_owned());
                for spec in inline_list(value) {
                    front.anchors.push(Anchor::parse(strip_quotes(spec.trim())));
                }
            }
            // A revision is derived, never stored: ignore a stale field rather
            // than reject a page the pool can still read.
            "rev" => block_key = None,
            other => {
                return Err(Error::Frontmatter(format!(
                    "unknown frontmatter key {other:?}; wayfinder accepts title, tags, kind, subject, depends, supersedes, created, updated, source, anchors"
                )));
            }
        }
    }
    if let Some(done) = anchor.take() {
        front.anchors.push(done);
    }
    if !closed {
        return Err(Error::Frontmatter(
            "opening `---` without a closing one".to_owned(),
        ));
    }
    front.anchors.retain(|a| !a.path.is_empty());
    Ok((front, body.trim_start_matches('\n').to_owned()))
}

/// Render frontmatter and body into the bytes a page file holds.
pub fn render(front: &Frontmatter, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 256);
    out.push_str("---\n");
    out.push_str(&format!("title: {}\n", quote(&front.title)));
    if !front.tags.is_empty() {
        out.push_str(&format!("tags: [{}]\n", front.tags.join(", ")));
    }
    if let Some(kind) = front.kind {
        out.push_str(&format!("kind: {}\n", kind.as_str()));
    }
    if !front.subjects.is_empty() {
        out.push_str(&format!("subject: [{}]\n", front.subjects.join(", ")));
    }
    if !front.depends.is_empty() {
        out.push_str(&format!("depends: [{}]\n", front.depends.join(", ")));
    }
    if let Some(supersedes) = &front.supersedes {
        out.push_str(&format!("supersedes: {supersedes}\n"));
    }
    if let Some(created) = &front.created {
        out.push_str(&format!("created: {created}\n"));
    }
    if let Some(updated) = &front.updated {
        out.push_str(&format!("updated: {updated}\n"));
    }
    if let Some(source) = &front.source {
        out.push_str(&format!("source: {source}\n"));
    }
    if !front.anchors.is_empty() {
        out.push_str("anchors:\n");
        for anchor in &front.anchors {
            match anchor.line {
                Some(line) => out.push_str(&format!("  - {}:{line}\n", anchor.path)),
                None => out.push_str(&format!("  - {}\n", anchor.path)),
            }
            if let Some(sha) = &anchor.sha {
                out.push_str(&format!("    sha: {sha}\n"));
            }
        }
    }
    out.push_str("---\n\n");
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

/// Content revision: sixteen hex characters over the canonical page bytes.
pub fn rev_of(front: &Frontmatter, body: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(render(front, body).as_bytes());
    hex16(&digest.finalize())
}

/// First sixteen hex characters of one digest.
pub fn hex16(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in bytes.iter().take(8) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// One section of a body: the heading line and everything under it until the
/// next heading of the same or a higher level.
pub fn section_of<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let wanted = heading.trim().to_ascii_lowercase();
    let mut start = None;
    let mut end = body.len();
    for (offset, line) in line_offsets(body) {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if level == 0 || level > 6 || !trimmed[level..].starts_with(' ') {
            continue;
        }
        match start {
            None => {
                if trimmed.to_ascii_lowercase().contains(&wanted) {
                    start = Some(offset);
                }
            }
            Some(_) if level <= 2 => {
                end = offset;
                break;
            }
            Some(_) => {}
        }
    }
    let start = start?;
    Some(body[start..end].trim())
}

/// Byte offsets of every line start, with the line text.
fn line_offsets(body: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        out.push((offset, line));
        offset += line.len();
    }
    out
}

/// Stable page id from a title: lowercase, ascii, dashes, bounded.
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_dash = true;
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "page".to_owned()
    } else {
        out
    }
}

/// Normalize a subject to `namespace:key`, lowercasing the namespace and
/// trimming separators a writer may have doubled.
pub fn normalize_subject(raw: &str) -> String {
    let trimmed = strip_quotes(raw).trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized = trimmed.replace(":/", ":").replace('/', ":");
    let normalized = normalized.trim_matches(':').replace("::", ":");
    match normalized.split_once(':') {
        Some((namespace, key)) if !key.is_empty() => {
            let key = key.trim_matches(':');
            format!("{}:{key}", namespace.trim().to_ascii_lowercase())
        }
        _ => format!("subject:{}", normalized.trim_matches(':')),
    }
}

/// Merge tags, keeping declaration order and one `run:` tag per batch.
pub fn merge_tags(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(existing.len() + incoming.len());
    for tag in existing.iter().chain(incoming) {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.starts_with("run:") && out.iter().any(|kept| kept.starts_with("run:")) {
            continue;
        }
        if !out.iter().any(|kept| kept == tag) {
            out.push(tag.to_owned());
        }
    }
    out
}

/// Strip one layer of matching quotes.
fn strip_quotes(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// Quote a scalar when the file needs it.
fn quote(value: &str) -> String {
    let needs = value.is_empty()
        || value.starts_with(['[', '{', '"', '\'', '#', '-', '*', '&', '!', '|', '>', '@'])
        || value.contains(": ")
        || value.ends_with(':')
        || value.contains(" #");
    if needs {
        format!("{:?}", value)
    } else {
        value.to_owned()
    }
}

/// Insert one non-empty value when it is new.
fn push_unique(list: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !list.iter().any(|kept| kept == value) {
        list.push(value.to_owned());
    }
}

/// Interpret `value` as an inline list, appending each entry.
fn extend_inline(list: &mut Vec<String>, value: &str, clean: impl Fn(&str) -> String) {
    for entry in inline_list(value) {
        let entry = clean(entry.trim());
        if !entry.is_empty() && !list.iter().any(|kept| *kept == entry) {
            list.push(entry);
        }
    }
}

/// Split `[a, b]` or `a, b` into its entries.
fn inline_list(value: &str) -> Vec<&str> {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Treat an empty scalar as absent.
fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
