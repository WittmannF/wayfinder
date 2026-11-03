//! Anchor verification: does the file a page cites still say what the page
//! claims it said?

use crate::error::Result;
use crate::page::{Anchor, Page, hex16};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Verdict for one anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorState {
    /// The file's bytes still hash to the recorded digest.
    Fresh,
    /// The file exists and differs from the recorded digest.
    Changed,
    /// The file is gone.
    Missing,
    /// The anchor names no digest, so nothing can be verified.
    Unverified,
}

impl AnchorState {
    /// Lowercase spelling used by the CLI and the graph.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Unverified => "unverified",
        }
    }
}

/// One anchor and the verdict it earned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorVerdict {
    /// The declared anchor.
    pub anchor: Anchor,
    /// What the filesystem says about it.
    pub state: AnchorState,
    /// Extra context, e.g. the digest that is there now.
    pub detail: Option<String>,
}

/// Check of one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// Page id.
    pub id: String,
    /// Page title.
    pub title: String,
    /// Page revision the check ran against.
    pub rev: String,
    /// One verdict per declared anchor, in declaration order.
    pub verdicts: Vec<AnchorVerdict>,
    /// Whether every anchor is `fresh`.
    pub all_fresh: bool,
}

impl CheckReport {
    /// Number of anchors that changed or went missing.
    pub fn drifted(&self) -> usize {
        self.verdicts
            .iter()
            .filter(|v| matches!(v.state, AnchorState::Changed | AnchorState::Missing))
            .count()
    }
}

/// Parse one `--anchor` argument into an anchor with a filled-in digest when
/// the file is present.
/// @param root - directory relative anchors resolve against
/// @param spec - `path`, `path:line`, or `path@<16 hex>`
/// @returns the anchor, with `sha` set from the file when it exists
pub fn anchor_spec(root: &Path, spec: &str) -> Anchor {
    let mut anchor = Anchor::parse(spec);
    if anchor.sha.is_none() {
        anchor.sha = file_digest(root, &anchor.path);
    }
    anchor
}

/// Digest of one file relative to `root`, when it can be read.
/// @param root - directory the path resolves against
/// @param path - repository-relative path
/// @returns sixteen hex characters, or `None` when the file cannot be read
pub fn file_digest(root: &Path, path: &str) -> Option<String> {
    let bytes = std::fs::read(root.join(path)).ok()?;
    Some(digest_of(&bytes))
}

/// Digest of some bytes, as anchors record it.
/// @param bytes - file content
/// @returns sixteen hex characters
pub fn digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex16(&hasher.finalize())
}

/// Verify every anchor of one page.
/// @param root - directory relative anchors resolve against
/// @param page - the page whose anchors are checked
/// @returns the report the CLI prints and the graph colors by
pub fn check_page(root: &Path, page: &Page) -> Result<CheckReport> {
    let mut verdicts = Vec::with_capacity(page.front.anchors.len());
    for anchor in &page.front.anchors {
        let Some(expected) = anchor.sha.as_deref() else {
            verdicts.push(AnchorVerdict {
                anchor: anchor.clone(),
                state: AnchorState::Unverified,
                detail: None,
            });
            continue;
        };
        let path = root.join(&anchor.path);
        if !path.exists() {
            verdicts.push(AnchorVerdict {
                anchor: anchor.clone(),
                state: AnchorState::Missing,
                detail: Some("the cited file is gone".to_owned()),
            });
            continue;
        }
        match file_digest(root, &anchor.path) {
            Some(current) if current == expected => verdicts.push(AnchorVerdict {
                anchor: anchor.clone(),
                state: AnchorState::Fresh,
                detail: None,
            }),
            Some(current) => verdicts.push(AnchorVerdict {
                anchor: anchor.clone(),
                state: AnchorState::Changed,
                detail: Some(format!("now {current}")),
            }),
            None => verdicts.push(AnchorVerdict {
                anchor: anchor.clone(),
                state: AnchorState::Missing,
                detail: Some("the cited file cannot be read".to_owned()),
            }),
        }
    }
    let all_fresh = verdicts.iter().all(|v| v.state == AnchorState::Fresh);
    Ok(CheckReport {
        id: page.id.clone(),
        title: page.front.title.clone(),
        rev: page.rev(),
        verdicts,
        all_fresh,
    })
}
