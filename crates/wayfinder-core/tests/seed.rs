//! Seeding: what the free pass derives from files, and how the priced pass
//! replaces it.

use std::fs;
use std::path::Path;
use wayfinder_core::{
    PageAnswer, PageRequest, PageWriter, Store, plan_pages, plan_structure, scan, seed_pages,
    seed_structure,
};

/// One workspace with a Rust crate, a Node package, a Python project, and a Go
/// module.
fn workspace() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    fs::create_dir_all(root.join("crates/alpha/src")).expect("alpha");
    fs::create_dir_all(root.join("web")).expect("web");
    fs::create_dir_all(root.join("py")).expect("py");
    fs::create_dir_all(root.join("go")).expect("go");
    fs::create_dir_all(root.join("node_modules/ignored")).expect("ignored");

    fs::write(
        root.join("crates/alpha/Cargo.toml"),
        "[package]\nname = \"alpha\"\ndescription = \"Limits requests per key.\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\ntokio = \"1\"\n",
    )
    .expect("cargo manifest");
    fs::write(root.join("crates/alpha/src/lib.rs"), "pub fn limit() {}\n").expect("source");
    fs::write(
        root.join("crates/alpha/README.md"),
        "# alpha\n\nRate limiting for the gateway.\n\nIt counts requests per key.\n",
    )
    .expect("readme");
    fs::write(
        root.join("web/package.json"),
        r#"{"name":"@demo/web","description":"The dashboard.","dependencies":{"react":"^18","vite":"^5"}}"#,
    )
    .expect("node manifest");
    fs::write(
        root.join("py/pyproject.toml"),
        "[project]\nname = \"demo-py\"\ndescription = \"Python side.\"\ndependencies = [\"httpx>=0.27\", \"ruff\"]\n",
    )
    .expect("python manifest");
    fs::write(
        root.join("go/go.mod"),
        "module example.com/demo/go-service\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n)\n",
    )
    .expect("go manifest");
    // A dependency tree and a build directory the walk must not enter.
    fs::write(
        root.join("node_modules/ignored/package.json"),
        r#"{"name":"inner-package"}"#,
    )
    .expect("ignored package");
    fs::create_dir_all(root.join("target/debug")).expect("target");
    fs::write(
        root.join("target/debug/Cargo.toml"),
        "[package]\nname = \"ghost\"\n",
    )
    .expect("build manifest");

    let store = Store::init(root).expect("init");
    (directory, store)
}

/// One page in the pool.
fn page_text(store: &Store, id: &str) -> String {
    fs::read_to_string(store.paths().page(id)).expect("page file")
}

#[test]
fn the_scan_finds_each_language_and_skips_dependency_trees() {
    let (_directory, store) = workspace();
    let packages = scan(store.root(), None);
    let names: Vec<&str> = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["@demo/web", "alpha", "demo-py", "go-service"],
        "{names:?}"
    );

    let alpha = packages
        .iter()
        .find(|package| package.name == "alpha")
        .expect("alpha");
    assert_eq!(alpha.directory, "crates/alpha");
    assert_eq!(alpha.manifest, "crates/alpha/Cargo.toml");
    assert_eq!(alpha.readme.as_deref(), Some("crates/alpha/README.md"));
    assert_eq!(
        alpha.description.as_deref(),
        Some("Limits requests per key.")
    );
    assert_eq!(alpha.dependencies, ["serde", "tokio"]);
    assert_eq!(alpha.page_id(), "package-alpha");
    assert_eq!(alpha.subjects(), ["package:alpha", "path:crates/alpha"]);

    let web = packages
        .iter()
        .find(|package| package.name == "@demo/web")
        .expect("web");
    assert_eq!(web.dependencies, ["react", "vite"]);
    let go = packages
        .iter()
        .find(|package| package.name == "go-service")
        .expect("go");
    assert_eq!(go.page_id(), "package-go-service");
}

#[test]
fn the_structure_pass_writes_one_pointer_page_per_package() {
    let (_directory, store) = workspace();
    let report = seed_structure(&store, None, "structure-1").expect("seed");
    assert_eq!(report.written, 4);
    assert_eq!(report.updated, 0);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.pages.len(), 4);

    let page = store.read("package-alpha").expect("alpha page");
    assert_eq!(page.front.kind, Some(wayfinder_core::Kind::Pointer));
    assert_eq!(page.front.subjects, ["package:alpha", "path:crates/alpha"]);
    assert_eq!(page.front.depends, ["serde", "tokio"]);
    assert_eq!(page.front.anchors.len(), 2);
    assert!(page.front.anchors.iter().all(|anchor| anchor.sha.is_some()));
    assert!(page.front.tags.iter().any(|tag| tag == "run:structure-1"));
    assert!(
        page.body.contains("Rate limiting for the gateway."),
        "{}",
        page.body
    );
    assert!(page.body.contains("## Declared dependencies"));
    assert!(page.body.contains("## Status"));

    // The build directory was not seeded.
    assert!(store.read("package-ghost").is_err());
    // Every page is checkable: it cites the files it was derived from.
    let report = wayfinder_core::check_page(store.root(), &page).expect("check");
    assert!(report.all_fresh);
}

#[test]
fn a_second_structure_run_refreshes_and_leaves_written_pages_alone() {
    let (_directory, store) = workspace();
    seed_structure(&store, None, "structure-1").expect("first");
    // A model-written page must survive the free pass.
    store
        .save(&wayfinder_core::SaveRequest {
            kind: Some(wayfinder_core::Kind::Fact),
            ..wayfinder_core::SaveRequest::new(
                "Package alpha",
                "## What it is\nProse a model wrote.",
            )
        })
        .expect("upgrade");

    let plan = plan_structure(&store, None).expect("plan");
    assert_eq!(plan.upgraded, ["alpha"]);

    let report = seed_structure(&store, None, "structure-2").expect("second");
    assert_eq!(report.updated, 3);
    assert_eq!(report.skipped, 1);
    let page = store.read("package-alpha").expect("alpha");
    assert_eq!(page.front.kind, Some(wayfinder_core::Kind::Fact));
    assert!(page.body.contains("Prose a model wrote."));
    // The refreshed pages carry the newer run tag.
    let beta = page_text(&store, "package-demo-py");
    assert!(beta.contains("run:structure-2"), "{beta}");
}

#[test]
fn the_priced_plan_states_what_a_run_would_spend() {
    let (_directory, store) = workspace();
    seed_structure(&store, None, "structure-1").expect("seed");

    let without_model = plan_pages(&store, None, 60_000, None);
    assert_eq!(without_model.packages, 4);
    assert_eq!(without_model.estimated_tokens, 240_000);
    assert!(without_model.unavailable.is_some());
    assert!(
        without_model
            .unavailable
            .expect("reason")
            .contains("--model")
    );

    let with_model = plan_pages(&store, Some(2), 1_500, Some("qwen3.8-27b".to_owned()));
    assert_eq!(with_model.packages, 2);
    assert_eq!(with_model.estimated_tokens, 3_000);
    assert_eq!(with_model.model.as_deref(), Some("qwen3.8-27b"));
    assert!(with_model.unavailable.is_none());
}

/// A writer that answers from a fixture, so the run is testable without a model.
struct StubWriter {
    body: String,
    fail_on: Option<String>,
}

impl PageWriter for StubWriter {
    fn write(&self, request: &PageRequest) -> wayfinder_core::Result<PageAnswer> {
        if self.fail_on.as_deref() == Some(request.package.name.as_str()) {
            return Err(wayfinder_core::Error::Invalid(
                "the endpoint refused".to_owned(),
            ));
        }
        Ok(PageAnswer {
            body: self.body.clone(),
            tokens: 1_200,
        })
    }
}

#[test]
fn the_page_pass_replaces_pointers_and_reports_the_spend() {
    let (_directory, store) = workspace();
    seed_structure(&store, Some(2), "structure-1").expect("seed");
    let writer = StubWriter {
        body: "## What it is\nThe gateway's limiter.\n\n## How it works\nIt counts per key.\n"
            .to_owned(),
        fail_on: None,
    };
    let report = seed_pages(&store, &writer, Some(2), "pages-1").expect("pages");
    assert_eq!(report.saved, 2);
    assert_eq!(report.failed, 0);
    assert_eq!(report.tokens_spent, 2_400);
    assert!(report.last_failure.is_none());

    let upgraded: Vec<_> = report
        .pages
        .iter()
        .map(|id| store.read(id).expect("page"))
        .collect();
    assert!(
        upgraded
            .iter()
            .all(|page| page.front.kind == Some(wayfinder_core::Kind::Fact))
    );
    let page = store.read("package-alpha").expect("alpha");
    assert!(page.body.contains("It counts per key."), "{}", page.body);
    assert!(page.front.tags.iter().any(|tag| tag == "run:pages-1"));
    assert_eq!(page.front.source.as_deref(), Some("pages:pages-1"));
    // The anchors from the structure pass are re-recorded, so drift still works.
    assert_eq!(
        wayfinder_core::check_page(store.root(), &page)
            .expect("check")
            .drifted(),
        0
    );
}

#[test]
fn a_failing_package_is_recorded_and_the_run_continues() {
    let (_directory, store) = workspace();
    seed_structure(&store, Some(2), "structure-1").expect("seed");
    let writer = StubWriter {
        body: "## What it is\nok\n".to_owned(),
        fail_on: Some("@demo/web".to_owned()),
    };
    let report = seed_pages(&store, &writer, Some(2), "pages-1").expect("pages");
    assert_eq!(report.saved, 1);
    assert_eq!(report.failed, 1);
    assert!(report.last_failure.expect("reason").contains("refused"));
}

#[test]
fn the_page_pass_skips_pages_it_did_not_write() {
    let (_directory, store) = workspace();
    store
        .save(&wayfinder_core::SaveRequest {
            kind: Some(wayfinder_core::Kind::Decision),
            ..wayfinder_core::SaveRequest::new("Package alpha", "A person wrote this by hand.")
        })
        .expect("hand-written page");
    let writer = StubWriter {
        body: "## What it is\nfrom a model\n".to_owned(),
        fail_on: None,
    };
    let report = seed_pages(&store, &writer, None, "pages-1").expect("pages");
    // The hand-written page is not a pointer, so the pass leaves it alone.
    assert_eq!(report.skipped, 1);
    assert!(!report.pages.contains(&"package-alpha".to_owned()));
    let page = store.read("package-alpha").expect("alpha");
    assert!(page.body.contains("A person wrote this by hand."));
    assert_eq!(page.front.kind, Some(wayfinder_core::Kind::Decision));
    // The packages that had no page at all are written.
    assert_eq!(report.saved, 3);
}

#[test]
fn a_workspace_without_manifests_seeds_nothing() {
    let directory = tempfile::tempdir().expect("temp dir");
    fs::write(directory.path().join("notes.md"), "prose only\n").expect("notes");
    let store = Store::init(directory.path()).expect("init");
    let plan = plan_structure(&store, None).expect("plan");
    assert!(plan.empty);
    let report = seed_structure(&store, None, "structure-1").expect("seed");
    assert_eq!(report.written, 0);
    assert!(store.pages().expect("pages").is_empty());
}

#[test]
fn a_readme_is_summarized_up_to_its_first_paragraph() {
    let (_directory, store) = workspace();
    seed_structure(&store, None, "structure-1").expect("seed");
    let page = store.read("package-alpha").expect("alpha");
    assert!(page.body.contains("Rate limiting for the gateway."));
    assert!(
        !page.body.contains("It counts requests per key."),
        "second paragraph belongs to the model pass"
    );
}

#[test]
fn the_walk_stops_at_the_configured_depth() {
    let directory = tempfile::tempdir().expect("temp dir");
    let deep = directory.path().join("a/b/c/d/e/f");
    fs::create_dir_all(&deep).expect("deep");
    fs::write(deep.join("package.json"), r#"{"name":"too-deep"}"#).expect("manifest");
    let store = Store::init(directory.path()).expect("init");
    assert!(scan(store.root(), None).is_empty());
}

/// Paths stay usable in a subject: a directory keeps its separators.
#[test]
fn a_path_subject_keeps_its_separators() {
    let (_directory, store) = workspace();
    seed_structure(&store, None, "structure-1").expect("seed");
    let page = store.read("package-alpha").expect("alpha");
    assert!(
        page.front
            .subjects
            .contains(&"path:crates/alpha".to_owned())
    );
    let graph = wayfinder_core::build_graph(&store, wayfinder_core::GraphBounds::default(), None)
        .expect("graph");
    let ids: Vec<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    assert!(ids.contains(&"subject:path:crates/alpha"), "{ids:?}");
    let _ = Path::new(".");
}
