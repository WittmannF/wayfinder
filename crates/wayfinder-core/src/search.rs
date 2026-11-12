//! Search: full-text ranking, then page-level relations and shared graph nodes
//! so a page that never names the query can still answer it.

use crate::error::Result;
use crate::index::SearchFilters;
use crate::page::{Kind, section_of};
use crate::store::Store;
use std::collections::BTreeMap;

/// Reciprocal-rank constant: the usual 60 keeps a deep neighbour from
/// outranking a direct match.
const RRF_K: f64 = 60.0;

/// Bytes of page text a hit carries back.
const SNIPPET_BYTES: usize = 400;

/// One search hit, with everything the caller needs to decide what to read.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Page id.
    pub id: String,
    /// Page title.
    pub title: String,
    /// What the page is for.
    pub kind: Option<Kind>,
    /// Tags as declared.
    pub tags: Vec<String>,
    /// Last update stamp.
    pub updated: Option<String>,
    /// Content revision, for a later compare-and-set save.
    pub rev: String,
    /// Fused relevance score.
    pub score: f64,
    /// The matched text, trimmed to a snippet.
    pub snippet: String,
    /// The relation that pulled this page in, when it was not a direct match.
    pub via: Option<String>,
    /// Number of anchors the page declares.
    pub anchors: usize,
}

/// What one search produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    /// The query as the caller wrote it.
    pub query: String,
    /// Pages, best first.
    pub pages: Vec<Hit>,
    /// How many of those came from a declared relation instead of the query.
    pub expanded: usize,
}

/// One section of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Heading the section starts at, when one matched.
    pub heading: Option<String>,
    /// Section text, heading included.
    pub text: String,
}

/// One page read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    /// The page as stored.
    pub page: crate::page::Page,
    /// The text the caller asked for: a section, or the whole body.
    pub section: Section,
    /// Whether the requested section was found.
    pub matched: bool,
}

impl Store {
    /// Search the pool, then widen to pages connected directly or through one
    /// shared subject, cited path, or dependency node.
    /// @param query - free text; empty lists the most recent pages
    /// @param filters - kind and subject narrowing
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<SearchOutcome> {
        let index = self.index()?;
        index.sync(self.paths())?;
        let direct = index.search(query, filters)?;
        let seeds: Vec<String> = direct.iter().map(|hit| hit.id.clone()).collect();
        let mut neighbours = index.neighbours(&seeds)?;
        let related_limit = filters.limit.unwrap_or(8).saturating_mul(4).clamp(16, 64);
        neighbours.extend(index.related_pages(&seeds, filters, related_limit)?);

        // Reciprocal rank fusion: a direct match keeps its rank, a neighbour
        // starts below every direct match and is damped by the predicate.
        let mut fused: BTreeMap<String, (f64, Option<String>)> = BTreeMap::new();
        for (rank, hit) in direct.iter().enumerate() {
            fused.insert(hit.id.clone(), (1.0 / (RRF_K + rank as f64 + 1.0), None));
        }
        let mut neighbour_rank = 0usize;
        for (id, predicate) in neighbours {
            if fused.contains_key(&id) {
                continue;
            }
            neighbour_rank += 1;
            let weight = match predicate.as_str() {
                "supersedes" | "depends" => 1.0,
                "shared-depends" => 0.75,
                "shared-cites" => 0.6,
                _ => 0.5,
            };
            fused.insert(
                id,
                (
                    weight / (RRF_K + direct.len() as f64 + neighbour_rank as f64 + 1.0),
                    Some(predicate),
                ),
            );
        }

        let mut ranked: Vec<(String, f64, Option<String>)> = fused
            .into_iter()
            .map(|(id, (score, via))| (id, score, via))
            .collect();
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });

        let limit = filters.limit.unwrap_or(8).max(1);
        let mut pages = Vec::with_capacity(ranked.len().min(limit));
        for (id, score, via) in ranked.into_iter().take(limit) {
            let page = match self.read(&id) {
                Ok(page) => page,
                Err(_) => continue,
            };
            if filters
                .kind
                .is_some_and(|kind| page.front.kind != Some(kind))
                || filters
                    .subject
                    .as_ref()
                    .is_some_and(|subject| !page.front.subjects.contains(subject))
            {
                continue;
            }
            pages.push(Hit {
                snippet: snippet_of(&page.body, query),
                rev: page.rev(),
                anchors: page.front.anchors.len(),
                id,
                title: page.front.title,
                kind: page.front.kind,
                tags: page.front.tags,
                updated: page.front.updated,
                score,
                via,
            });
        }
        let expanded = pages.iter().filter(|hit| hit.via.is_some()).count();
        Ok(SearchOutcome {
            query: query.to_owned(),
            pages,
            expanded,
        })
    }

    /// Read one page, optionally narrowed to one section.
    /// @param id - page id
    /// @param section - heading to return; absent reads the whole body
    pub fn read_section(&self, id: &str, section: Option<&str>) -> Result<ReadOutcome> {
        let page = self.read(id)?;
        let section = match section {
            Some(heading) if !heading.trim().is_empty() => match section_of(&page.body, heading) {
                Some(text) => Section {
                    heading: Some(heading.to_owned()),
                    text: text.to_owned(),
                },
                None => Section {
                    heading: None,
                    text: page.body.clone(),
                },
            },
            _ => Section {
                heading: None,
                text: page.body.clone(),
            },
        };
        let matched = section.heading.is_some();
        Ok(ReadOutcome {
            page,
            section,
            matched,
        })
    }
}

/// First useful chunk of a body: the section mentioning the query when one
/// does, otherwise the opening lines.
fn snippet_of(body: &str, query: &str) -> String {
    let lowered = body.to_ascii_lowercase();
    let first_word = query
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .find(|word| word.len() > 2);
    let start = first_word
        .and_then(|word| lowered.find(&word))
        .map(|offset| {
            // Back up to the line that carries the match so the snippet reads
            // as prose rather than starting mid-word.
            body[..offset].rfind('\n').map_or(0, |line| line + 1)
        })
        .unwrap_or(0);
    let tail = &body[start..];
    let mut end = SNIPPET_BYTES.min(tail.len());
    while end < tail.len() && !tail.is_char_boundary(end) {
        end -= 1;
    }
    let slice = &tail[..end];
    let trimmed = slice.trim();
    if tail.len() > end {
        format!("{trimmed}…")
    } else {
        trimmed.to_owned()
    }
}
