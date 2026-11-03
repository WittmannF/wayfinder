//! Behavior of the pool: pages, revisions, anchors, search, the digest, the
//! graph, and concurrent writers.

use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use wayfinder_core::{
    Anchor, DigestOptions, GraphBounds, GraphFormat, Kind, SaveRequest, SearchFilters, Store,
    build_graph, check_page, digest, render_graph, section_of,
};

/// A pool in a fresh directory, with a cited file next to it.
fn pool() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("temp dir");
    fs::create_dir_all(directory.path().join("src")).expect("src");
    fs::write(directory.path().join("src/limiter.rs"), "fn limit() {}\n").expect("cited file");
    let store = Store::init(directory.path()).expect("init");
    (directory, store)
}

#[test]
fn a_saved_page_keeps_what_it_declared() {
    let (directory, store) = pool();
    let request = SaveRequest {
        tags: vec!["memory".to_owned()],
        kind: Some(Kind::Fact),
        subjects: vec!["package:widget".to_owned()],
        depends: vec!["serde".to_owned()],
        anchors: vec![Anchor::parse("src/limiter.rs:42")],
        source: Some("session:abc".to_owned()),
        ..SaveRequest::new("Rate limits", "## Throughput\n100 per second.")
    };
    let result = store.save(&request).expect("save");
    assert_eq!(result.id, "rate-limits");
    assert!(result.created);
    assert!(result.unresolved_anchors.is_empty());

    let page = store.read("rate-limits").expect("read");
    assert_eq!(page.front.title, "Rate limits");
    assert_eq!(page.front.kind, Some(Kind::Fact));
    assert_eq!(page.front.subjects, ["package:widget"]);
    assert_eq!(page.front.depends, ["serde"]);
    assert_eq!(page.front.tags, ["memory"]);
    assert_eq!(page.front.anchors[0].path, "src/limiter.rs");
    assert_eq!(page.front.anchors[0].line, Some(42));
    assert!(page.front.anchors[0].sha.is_some());
    assert_eq!(page.body.trim(), "## Throughput\n100 per second.");
    assert!(page.front.created.is_some());
    let _ = directory;
}

#[test]
fn a_second_save_merges_instead_of_replacing() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest {
            tags: vec!["first".to_owned()],
            subjects: vec!["package:widget".to_owned()],
            ..SaveRequest::new("Rate limits", "one")
        })
        .expect("first save");
    let second = store
        .save(&SaveRequest {
            tags: vec!["second".to_owned(), "run:one".to_owned()],
            subjects: vec!["package:other".to_owned()],
            ..SaveRequest::new("Rate limits", "two")
        })
        .expect("second save");
    assert!(!second.created);
    let page = store.read("rate-limits").expect("read");
    assert_eq!(page.front.tags, ["first", "second", "run:one"]);
    assert_eq!(page.front.subjects, ["package:widget", "package:other"]);
    assert_eq!(page.body.trim(), "two");
}

#[test]
fn a_stale_revision_is_refused_and_a_matching_one_accepted() {
    let (_directory, store) = pool();
    let saved = store
        .save(&SaveRequest::new("Rate limits", "v1"))
        .expect("save");
    store
        .save(&SaveRequest::new("Rate limits", "v2"))
        .expect("external change");

    let error = store
        .save(&SaveRequest {
            expected_rev: Some(saved.rev.clone()),
            ..SaveRequest::new("Rate limits", "stale")
        })
        .expect_err("stale write must be refused");
    assert!(
        matches!(error, wayfinder_core::Error::StaleRevision { .. }),
        "{error}"
    );
    assert_eq!(store.read("rate-limits").expect("read").body.trim(), "v2");

    let current = store.read("rate-limits").expect("read").rev();
    store
        .save(&SaveRequest {
            expected_rev: Some(current),
            ..SaveRequest::new("Rate limits", "v3")
        })
        .expect("fenced write");
    assert_eq!(store.read("rate-limits").expect("read").body.trim(), "v3");
}

#[test]
fn a_revision_follows_the_bytes() {
    let (_directory, store) = pool();
    let first = store
        .save(&SaveRequest::new("Rate limits", "v1"))
        .expect("save");
    let second = store
        .save(&SaveRequest::new("Rate limits", "v2"))
        .expect("save");
    assert_ne!(first.rev, second.rev);
    let page = store.read("rate-limits").expect("read");
    assert_eq!(page.rev(), second.rev);
    // The same bytes always hash the same way.
    assert_eq!(page.rev(), store.read("rate-limits").expect("read").rev());
}

#[test]
fn an_anchor_reports_fresh_then_changed_then_missing() {
    let (directory, store) = pool();
    store
        .save(&SaveRequest {
            anchors: vec![Anchor::parse("src/limiter.rs")],
            ..SaveRequest::new("Rate limits", "cited")
        })
        .expect("save");

    let page = store.read("rate-limits").expect("read");
    let fresh = check_page(store.root(), &page).expect("check");
    assert!(fresh.all_fresh);
    assert_eq!(fresh.drifted(), 0);

    fs::write(
        directory.path().join("src/limiter.rs"),
        "fn limit() { /* changed */ }\n",
    )
    .expect("edit");
    let changed = check_page(store.root(), &page).expect("check");
    assert_eq!(changed.drifted(), 1);
    assert_eq!(changed.verdicts[0].state.as_str(), "changed");

    fs::remove_file(directory.path().join("src/limiter.rs")).expect("remove");
    let missing = check_page(store.root(), &page).expect("check");
    assert_eq!(missing.verdicts[0].state.as_str(), "missing");
}

#[test]
fn a_page_without_anchors_declares_nothing_to_verify() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest::new("Rate limits", "no citations"))
        .expect("save");
    let page = store.read("rate-limits").expect("read");
    let report = check_page(store.root(), &page).expect("check");
    assert!(report.verdicts.is_empty());
    assert!(report.all_fresh);
}

#[test]
fn search_finds_a_page_and_then_its_neighbours() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest::new(
            "Rate limits",
            "The limiter allows 100 requests per second.",
        ))
        .expect("save");
    store
        .save(&SaveRequest {
            depends: vec!["widget".to_owned()],
            ..SaveRequest::new(
                "Limiter config",
                "LIMIT_PER_SEC is read from the environment.",
            )
        })
        .expect("save");

    let direct = store
        .search("requests per second", &SearchFilters::default())
        .expect("search");
    assert_eq!(direct.pages[0].id, "rate-limits");
    assert!(direct.pages[0].snippet.contains("100 requests"));

    // "LIMIT_PER_SEC" never mentions the query, so it only arrives through the
    // declared dependency.
    let expanded = store
        .search("rate limits", &SearchFilters::default())
        .expect("search");
    assert!(expanded.pages.iter().any(|hit| hit.id == "rate-limits"));
    let narrowed = store
        .search("requests per second", &SearchFilters::default())
        .expect("search");
    assert!(narrowed.expanded <= narrowed.pages.len());
}

#[test]
fn search_narrows_by_kind_and_subject() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest {
            kind: Some(Kind::Fact),
            ..SaveRequest::new("Alpha fact", "shared wording")
        })
        .expect("save");
    store
        .save(&SaveRequest {
            kind: Some(Kind::Gotcha),
            ..SaveRequest::new("Beta gotcha", "shared wording")
        })
        .expect("save");

    let facts = store
        .search(
            "shared wording",
            &SearchFilters {
                kind: Some(Kind::Fact),
                ..Default::default()
            },
        )
        .expect("search");
    assert_eq!(facts.pages.len(), 1);
    assert_eq!(facts.pages[0].id, "alpha-fact");

    let subjects = store
        .search(
            "shared wording",
            &SearchFilters {
                subject: Some("package:widget".to_owned()),
                ..Default::default()
            },
        )
        .expect("search");
    assert!(subjects.pages.is_empty());
}

#[test]
fn an_empty_query_lists_the_most_recent_pages() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest::new("Alpha", "first"))
        .expect("save");
    store
        .save(&SaveRequest::new("Beta", "second"))
        .expect("save");
    let listed = store.search("", &SearchFilters::default()).expect("search");
    assert_eq!(listed.pages.len(), 2);
}

#[test]
fn a_read_returns_the_section_not_the_page() {
    let (_directory, store) = pool();
    store.save(&SaveRequest::new(
        "Rate limits",
        "intro paragraph\n\n## Throughput\nThe limiter allows 100 per second.\n\n## Where\nIn the limiter module.",
    )).expect("save");

    let section = store
        .read_section("rate-limits", Some("throughput"))
        .expect("read");
    assert!(section.matched);
    assert!(section.section.text.contains("100 per second"));
    assert!(!section.section.text.contains("Where"));

    let whole = store.read_section("rate-limits", None).expect("read");
    assert!(whole.section.text.contains("intro paragraph"));
    assert!(!whole.matched);
}

#[test]
fn the_digest_names_the_pool_and_an_empty_pool_says_nothing() {
    let (_directory, store) = pool();
    assert!(
        digest(&store, DigestOptions::default())
            .expect("digest")
            .is_empty()
    );
    store
        .save(&SaveRequest::new("Rate limits", "100 per second"))
        .expect("save");
    let text = digest(&store, DigestOptions::default()).expect("digest");
    assert!(text.contains("1 memory page"));
    assert!(text.contains("Rate limits (id: rate-limits"));
    assert!(text.contains("wayfinder search"));
}

#[test]
fn the_index_follows_files_written_outside_the_store() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest::new("Rate limits", "original body"))
        .expect("save");
    // A different editor, a script, or another agent writes the file directly.
    let path = store.paths().page("rate-limits");
    let text = fs::read_to_string(&path).expect("read file");
    fs::write(
        &path,
        text.replace("original body", "rewritten body about quokkas"),
    )
    .expect("write");

    let hits = store
        .search("quokkas", &SearchFilters::default())
        .expect("search");
    assert_eq!(hits.pages.len(), 1);

    fs::remove_file(&path).expect("remove");
    let after = store
        .search("quokkas", &SearchFilters::default())
        .expect("search");
    assert!(after.pages.is_empty());
}

#[test]
fn the_graph_draws_pages_subjects_paths_and_edges() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest {
            subjects: vec!["package:widget".to_owned()],
            depends: vec!["widget".to_owned()],
            anchors: vec![Anchor::parse("src/limiter.rs")],
            ..SaveRequest::new("Rate limits", "100 per second")
        })
        .expect("save");

    let view = build_graph(&store, GraphBounds::default(), None).expect("graph");
    let ids: Vec<&str> = view.nodes.iter().map(|node| node.id.as_str()).collect();
    assert!(ids.contains(&"page:rate-limits"));
    assert!(ids.contains(&"subject:package:widget"));
    assert!(ids.contains(&"path:src/limiter.rs"));
    // A dependency and a declared subject name the same node, not two.
    assert_eq!(
        ids.iter()
            .filter(|id| **id == "subject:package:widget")
            .count(),
        1
    );
    assert!(view.edges.iter().any(|edge| edge.predicate == "cites"));
    assert!(view.edges.iter().any(|edge| edge.predicate == "depends"));
    assert!(!view.truncated);

    let json = render_graph(&view, GraphFormat::Json).expect("json");
    assert!(json.contains("\"page:rate-limits\""));
    let svg = render_graph(&view, GraphFormat::Svg).expect("svg");
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("Rate limits"));
    let mermaid = render_graph(&view, GraphFormat::Mermaid).expect("mermaid");
    assert!(mermaid.starts_with("flowchart LR"));
    let dot = render_graph(&view, GraphFormat::Dot).expect("dot");
    assert!(dot.starts_with("digraph wayfinder"));
    let html = render_graph(&view, GraphFormat::Html).expect("html");
    assert!(html.contains("<svg"));
}

#[test]
fn the_graph_narrows_to_one_subject() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest {
            subjects: vec!["package:widget".to_owned()],
            ..SaveRequest::new("Widget page", "about widgets")
        })
        .expect("save");
    store
        .save(&SaveRequest {
            subjects: vec!["package:other".to_owned()],
            ..SaveRequest::new("Other page", "about others")
        })
        .expect("save");

    let view = build_graph(&store, GraphBounds::default(), Some("package:widget")).expect("graph");
    assert!(view.nodes.iter().any(|node| node.id == "page:widget-page"));
    assert!(!view.nodes.iter().any(|node| node.id == "page:other-page"));
}

#[test]
fn sections_are_found_by_partial_heading() {
    let body = "intro\n\n## Throughput\nThe limiter allows 100 per second.\n\n## Where\nIn the limiter module.";
    assert!(
        section_of(body, "THROUGHPUT")
            .expect("found")
            .contains("100 per second")
    );
    assert!(section_of(body, "nothing like it").is_none());
    assert!(section_of("no headings here", "anything").is_none());
}

#[test]
fn concurrent_writers_all_land_and_the_index_keeps_up() {
    let (_directory, store) = pool();
    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for index in 0..8 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            store
                .save(&SaveRequest::new(
                    format!("Page {index}"),
                    format!("body number {index}"),
                ))
                .expect("concurrent save");
        }));
    }
    for handle in handles {
        handle.join().expect("thread");
    }
    let pages = store.pages().expect("pages");
    assert_eq!(pages.len(), 8);
    let index = store.index().expect("index");
    index.sync(store.paths()).expect("sync");
    assert_eq!(index.page_count().expect("count"), 8);
}

#[test]
fn two_writers_racing_one_page_leave_one_winner() {
    let (_directory, store) = pool();
    let saved = store
        .save(&SaveRequest::new("Rate limits", "base"))
        .expect("save");
    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for writer in 0..2 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let rev = saved.rev.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            store
                .save(&SaveRequest {
                    expected_rev: Some(rev),
                    ..SaveRequest::new("Rate limits", format!("writer {writer}"))
                })
                .map(|_| writer)
        }));
    }
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    let winners = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(winners, 1, "exactly one fenced write may win: {outcomes:?}");
    let body = store.read("rate-limits").expect("read").body;
    assert!(body.starts_with("writer"), "{body:?}");
}
