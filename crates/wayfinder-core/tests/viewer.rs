//! The viewer: one self-contained file that a reader can click through.

use std::fs;
use wayfinder_core::{
    Anchor, GraphBounds, GraphFormat, SaveRequest, Store, build_graph, render_graph,
};

/// One pool with two pages, a shared subject, a cited file, and a tag.
fn pool() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("temp dir");
    fs::create_dir_all(directory.path().join("src")).expect("src");
    fs::write(directory.path().join("src/limiter.rs"), "fn limit() {}\n").expect("cited file");
    let store = Store::init(directory.path()).expect("init");
    store
        .save(&SaveRequest {
            kind: Some(wayfinder_core::Kind::Fact),
            subjects: vec!["package:widget".to_owned()],
            tags: vec!["memory".to_owned()],
            anchors: vec![Anchor::parse("src/limiter.rs")],
            ..SaveRequest::new(
                "Rate limits",
                "## Throughput\nThe limiter allows 100 per second.",
            )
        })
        .expect("first page");
    store
        .save(&SaveRequest {
            kind: Some(wayfinder_core::Kind::Gotcha),
            subjects: vec!["package:widget".to_owned()],
            depends: vec!["widget".to_owned()],
            ..SaveRequest::new("Limiter config", "LIMIT_PER_SEC is read at boot.")
        })
        .expect("second page");
    (directory, store)
}

/// Render the viewer for one pool.
fn viewer(store: &Store, bounds: GraphBounds) -> String {
    let view = build_graph(store, bounds, None).expect("graph");
    render_graph(&view, GraphFormat::Html).expect("html")
}

#[test]
fn the_viewer_is_one_self_contained_file_with_its_controls() {
    let (_directory, store) = pool();
    let html = viewer(&store, GraphBounds::default());

    // Nothing is fetched: the page works from disk with no network.
    assert!(!html.contains("<script src"), "{html}");
    assert!(
        !html.contains("https://"),
        "no remote asset may be referenced"
    );
    let without_namespaces = html.replace("http://www.w3.org/2000/svg", "");
    assert!(
        !without_namespaces.contains("http://"),
        "no remote asset may be referenced"
    );

    // The data, the controls, and the panel the script drives.
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains(r#"id="wayfinder-graph""#));
    assert!(html.contains(r#"id="query""#));
    assert!(html.contains(r#"id="kinds""#));
    assert!(html.contains(r#"id="subject""#));
    assert!(html.contains(r#"id="detail""#));
    assert!(html.contains(r#"role="region""#));
    assert!(html.contains(r#"aria-live="polite""#));
    assert!(html.contains("anchors match"));
    assert!(html.contains("cited file changed"));
    // The graph itself is embedded, with the facts the panel shows.
    assert!(html.contains("Rate limits"));
    assert!(html.contains("The limiter allows 100 per second."));
    assert!(html.contains("\"freshness\":\"fresh\""));
}

#[test]
fn a_label_cannot_close_the_embedded_data_block() {
    let directory = tempfile::tempdir().expect("temp dir");
    let store = Store::init(directory.path()).expect("init");
    store
        .save(&SaveRequest::new(
            "evil </script><script>alert(1)</script>",
            "body",
        ))
        .expect("page");
    let html = viewer(&store, GraphBounds::default());
    // The literal sequence appears once, as the data block's terminator would:
    // the payload itself escapes every `</`.
    let closings = html.matches("</script>").count();
    assert_eq!(
        closings, 2,
        "only the document's own two script tags may close"
    );
    assert!(
        html.contains("<\\/script"),
        "the payload must escape its own closers"
    );
}

#[test]
fn a_page_beyond_the_budget_joins_its_hub_cluster() {
    let (_directory, store) = pool();
    let bounds = GraphBounds {
        pages: 1,
        nodes: 200,
        edges: 400,
    };
    let view = build_graph(&store, bounds, None).expect("graph");
    assert!(view.truncated);
    let hub = view
        .nodes
        .iter()
        .find(|node| node.id == "subject:package:widget")
        .expect("hub keeps its place");
    assert_eq!(hub.count, Some(1), "one page was hidden behind the budget");
    // Pages are placed in id order, so `rate-limits` is the one behind the budget.
    assert_eq!(
        hub.hidden.as_deref(),
        Some(["rate-limits".to_owned()].as_slice()),
        "the cluster names the page it stands for"
    );
    // The drawn page keeps its own node.
    assert!(
        view.nodes
            .iter()
            .any(|node| node.id == "page:limiter-config")
    );
    // The viewer renders the count so a reader can see something was hidden.
    let html = render_graph(&view, GraphFormat::Html).expect("html");
    assert!(html.contains("\"count\":1"));
    assert!(html.contains("pages hidden by the node budget"));
}

#[test]
fn a_superseded_page_is_flagged_for_the_reader() {
    let (_directory, store) = pool();
    store
        .save(&SaveRequest {
            supersedes: Some("rate-limits".to_owned()),
            ..SaveRequest::new("Rate limits, corrected", "The limit is now 50 per second.")
        })
        .expect("replacement");
    let view = build_graph(&store, GraphBounds::default(), None).expect("graph");
    let replaced = view
        .nodes
        .iter()
        .find(|node| node.id == "page:rate-limits")
        .expect("the replaced page still exists");
    assert_eq!(replaced.superseded, Some(true));
    let html = render_graph(&view, GraphFormat::Html).expect("html");
    assert!(html.contains("another page replaces this one"));
}

#[test]
fn the_banner_names_the_pages_whose_cited_file_changed() {
    let (directory, store) = pool();
    fs::write(
        directory.path().join("src/limiter.rs"),
        "fn limit() { /* moved */ }\n",
    )
    .expect("edit the cited file");
    let html = viewer(&store, GraphBounds::default());
    assert!(html.contains("1 page cites a file that changed"), "{html}");
    assert!(html.contains("wayfinder check"));
}

#[test]
fn the_static_renderings_stay_available() {
    let (_directory, store) = pool();
    let view = build_graph(&store, GraphBounds::default(), None).expect("graph");
    assert!(
        render_graph(&view, GraphFormat::Svg)
            .expect("svg")
            .starts_with("<svg")
    );
    assert!(
        render_graph(&view, GraphFormat::Mermaid)
            .expect("mermaid")
            .starts_with("flowchart")
    );
    assert!(
        render_graph(&view, GraphFormat::Dot)
            .expect("dot")
            .starts_with("digraph")
    );
    let json = render_graph(&view, GraphFormat::Json).expect("json");
    assert!(json.contains("\"page:rate-limits\""));
}
