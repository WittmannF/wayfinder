//! Wayfinder core: workspace memory that agents write to and reuse.
//!
//! A pool is a directory of markdown pages. Each page carries frontmatter an
//! agent fills in (title, tags, kind, subjects, anchors, dependencies) and a
//! body a later session reads. The store owns the files; the index is derived
//! and rebuildable; revisions are content hashes so a stale writer loses.
//!
//! ```no_run
//! use wayfinder_core::{Store, SaveRequest};
//!
//! let store = Store::open(".").unwrap();
//! store.save(&SaveRequest::new("Rate limiter findings", "Bucket of 100 per second.")).unwrap();
//! let hits = store.search("rate limit", &Default::default()).unwrap();
//! assert_eq!(hits.pages.len(), 1);
//! ```

mod anchors;
mod digest;
mod error;
mod graph;
mod index;
mod page;
mod search;
mod seed;
mod store;
mod viewer;

pub use anchors::{AnchorState, AnchorVerdict, CheckReport, anchor_spec, check_page};
pub use digest::{DigestOptions, digest};
pub use error::{Error, Result};
pub use graph::{
    GraphBounds, GraphEdge, GraphFormat, GraphNode, GraphView, NodeKind, build_graph, layout,
    render_graph,
};
pub use index::{Index, IndexHit, SearchFilters, SyncStats};
pub use page::{Anchor, Frontmatter, Kind, Page, rev_of, section_of, slugify};
pub use search::{Hit, ReadOutcome, SearchOutcome, Section};
pub use seed::{
    Area, ManifestKind, Package, PageAnswer, PagePlan, PageReport, PageRequest, PageWriter,
    StructurePlan, StructureReport, StructureTarget, plan_pages, plan_structure, scan, seed_pages,
    seed_structure, structure_targets, subjects_in_use,
};
pub use store::{
    AnchorInput, SaveRequest, SaveResult, Store, StorePaths, default_root, now_rfc3339,
    resolve_root,
};
pub use viewer::render_viewer_html;
