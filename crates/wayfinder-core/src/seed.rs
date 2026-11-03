//! Seeding: a pool on day one.
//!
//! Two passes with different prices. The structure pass reads manifests and
//! READMEs and writes one pointer page per package; it calls no model. The page
//! pass replaces those pointers with model-written pages, one child per package,
//! behind a plan that states the cost first.

use crate::error::Result;
use crate::page::Kind;
use crate::store::{SaveRequest, Store};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Directories a walk never enters: build output, dependency trees, and the
/// pool itself.
const SKIPPED: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".wayfinder",
    ".dsh",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    "vendor",
    ".next",
    ".turbo",
    "coverage",
    ".idea",
    ".vscode",
    "tmp",
];

/// How deep the walk goes, relative to the root.
const MAX_DEPTH: usize = 4;

/// Size cap for a README a page quotes.
const README_BYTES: usize = 4_000;

/// A language manifest wayfinder understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    /// `Cargo.toml`
    Cargo,
    /// `package.json`
    Node,
    /// `pyproject.toml`
    Python,
    /// `go.mod`
    Go,
}

impl ManifestKind {
    /// File name that identifies this manifest.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Cargo => "Cargo.toml",
            Self::Node => "package.json",
            Self::Python => "pyproject.toml",
            Self::Go => "go.mod",
        }
    }

    /// Every manifest name the walk looks for.
    fn all() -> [Self; 4] {
        [Self::Cargo, Self::Node, Self::Python, Self::Go]
    }
}

/// One package the structure pass found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Name from the manifest.
    pub name: String,
    /// Directory relative to the root, with `/` separators.
    pub directory: String,
    /// Manifest that named it, relative to the root.
    pub manifest: String,
    /// README beside the manifest, when one exists.
    pub readme: Option<String>,
    /// One-line description from the manifest.
    pub description: Option<String>,
    /// Runtime dependencies the manifest declares.
    pub dependencies: Vec<String>,
}

impl Package {
    /// Page id the structure pass writes, and the page pass upgrades.
    pub fn page_id(&self) -> String {
        crate::page::slugify(&format!("package {}", self.name))
    }

    /// Subjects the page declares.
    pub fn subjects(&self) -> Vec<String> {
        let mut subjects = vec![format!("package:{}", self.name)];
        if !self.directory.is_empty() && self.directory != "." {
            subjects.push(format!("path:{}", self.directory));
        }
        subjects
    }
}

/// What a structure run would do.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StructurePlan {
    /// Packages found, in walk order.
    pub packages: Vec<String>,
    /// Packages whose page already exists and is not a pointer, which a re-run
    /// leaves alone.
    pub upgraded: Vec<String>,
    /// Whether anything would be written.
    pub empty: bool,
}

/// What a structure run did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StructureReport {
    /// Pages written for the first time.
    pub written: usize,
    /// Pages refreshed in place.
    pub updated: usize,
    /// Packages whose page already carries a model-written body.
    pub skipped: usize,
    /// Page ids written, in order.
    pub pages: Vec<String>,
}

/// An estimate the priced pass shows before it spends anything.
#[derive(Debug, Clone, Serialize)]
pub struct PagePlan {
    /// Packages that would be upgraded.
    pub packages: usize,
    /// Packages skipped because the pass already wrote them.
    pub skipped: usize,
    /// Tokens per package the estimate uses.
    pub token_estimate_per_package: u64,
    /// Total tokens the run is expected to spend.
    pub estimated_tokens: u64,
    /// Model the run would call, when one is configured.
    pub model: Option<String>,
    /// Why the pass cannot run, when it cannot.
    pub unavailable: Option<String>,
}

/// One request to write a page with a model.
#[derive(Debug, Clone)]
pub struct PageRequest {
    /// Package the page describes.
    pub package: Package,
    /// Files the model should read, relative to the root.
    pub files: Vec<String>,
    /// Prompt sent to the model.
    pub prompt: String,
}

/// One model answer.
#[derive(Debug, Clone)]
pub struct PageAnswer {
    /// The page body the model wrote.
    pub body: String,
    /// Tokens the provider reports for the call.
    pub tokens: u64,
}

/// Somewhere a page can be written.
pub trait PageWriter {
    /// Write one page body for one package.
    /// @param request - the package, its files, and the prompt
    fn write(&self, request: &PageRequest) -> Result<PageAnswer>;
}

/// What a priced run did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageReport {
    /// Packages upgraded.
    pub saved: usize,
    /// Packages whose page already carried a model-written body.
    pub skipped: usize,
    /// Packages whose writing failed.
    pub failed: usize,
    /// Tokens the provider reported.
    pub tokens_spent: u64,
    /// Reason the last failure carried.
    pub last_failure: Option<String>,
    /// Page ids written, in order.
    pub pages: Vec<String>,
}

/// Find the packages of one workspace.
/// @param root - project root to walk
/// @param limit - maximum packages to report
pub fn scan(root: &Path, limit: Option<usize>) -> Vec<Package> {
    let mut packages = Vec::new();
    walk(root, root, 0, &mut packages);
    // Sort before truncating: the filesystem's directory order is not a policy,
    // so both limits name the same packages on every machine.
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    if let Some(limit) = limit {
        packages.truncate(limit);
    }
    packages
}

/// Walk one directory, collecting packages.
fn walk(root: &Path, directory: &Path, depth: usize, out: &mut Vec<Package>) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if SKIPPED.contains(&name) || name.starts_with('.') && name != ".github" {
                continue;
            }
            children.push(path);
            continue;
        }
        let Some(kind) = ManifestKind::all()
            .into_iter()
            .find(|kind| kind.file_name() == name)
        else {
            continue;
        };
        if let Some(package) = read_package(root, directory, &path, kind) {
            out.push(package);
            // A package root holds its own manifest; keep walking for nested
            // workspaces but not below this manifest's own directory twice.
        }
    }
    for child in children {
        walk(root, &child, depth + 1, out);
    }
}

/// Read one manifest into a package.
fn read_package(
    root: &Path,
    directory: &Path,
    manifest: &Path,
    kind: ManifestKind,
) -> Option<Package> {
    let text = fs::read_to_string(manifest).ok()?;
    let (name, description, dependencies) = match kind {
        ManifestKind::Node => read_node_manifest(&text)?,
        ManifestKind::Cargo => read_cargo_manifest(&text)?,
        ManifestKind::Python => read_python_manifest(&text)?,
        ManifestKind::Go => read_go_module(&text)?,
    };
    if name.trim().is_empty() {
        return None;
    }
    let relative_directory = relative(root, directory);
    let readme = ["README.md", "README.rst", "README.txt", "readme.md"]
        .iter()
        .map(|candidate| directory.join(candidate))
        .find(|candidate| candidate.is_file())
        .map(|candidate| relative(root, &candidate));
    Some(Package {
        name: name.trim().to_owned(),
        directory: relative_directory,
        manifest: relative(root, manifest),
        readme,
        description,
        dependencies,
    })
}

/// Directory relative to the root, with `/` separators.
fn relative(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    let text = stripped.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}

/// Read a `package.json`.
fn read_node_manifest(text: &str) -> Option<(String, Option<String>, Vec<String>)> {
    let document: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = document.get("name")?.as_str()?.to_owned();
    let description = document
        .get("description")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|value| !value.is_empty());
    let dependencies = document
        .get("dependencies")
        .and_then(|value| value.as_object())
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    Some((name, description, dependencies))
}

/// Read the `[package]` table of a `Cargo.toml`, plus its dependency names.
fn read_cargo_manifest(text: &str) -> Option<(String, Option<String>, Vec<String>)> {
    let mut section = String::new();
    let mut name = None;
    let mut description = None;
    let mut dependencies = Vec::new();
    let mut in_dependencies = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = header.trim().to_owned();
            in_dependencies = section.starts_with("dependencies")
                || section.starts_with("dev-dependencies")
                || section.starts_with("build-dependencies")
                || section.contains("dependencies");
            continue;
        }
        if in_dependencies {
            if let Some(key) = trimmed.split('=').next() {
                let key = key.trim();
                if !key.is_empty() && !key.starts_with('#') {
                    dependencies.push(key.to_owned());
                }
            }
            continue;
        }
        if section == "package" {
            if let Some(value) = trimmed.strip_prefix("name") {
                if let Some(value) = value.trim_start().strip_prefix('=') {
                    name = Some(value.trim().trim_matches('"').to_owned());
                }
            }
            if let Some(value) = trimmed.strip_prefix("description") {
                if let Some(value) = value.trim_start().strip_prefix('=') {
                    description = Some(value.trim().trim_matches('"').to_owned());
                }
            }
        }
    }
    Some((
        name?,
        description.filter(|value| !value.is_empty()),
        dependencies,
    ))
}

/// Read a `pyproject.toml`.
fn read_python_manifest(text: &str) -> Option<(String, Option<String>, Vec<String>)> {
    let mut section = String::new();
    let mut name = None;
    let mut description = None;
    let mut dependencies = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = header.trim().to_owned();
            continue;
        }
        if !(section == "project" || section == "tool.poetry") {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                name = Some(value.trim().trim_matches('"').to_owned());
            }
        }
        if let Some(value) = trimmed.strip_prefix("description") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                description = Some(value.trim().trim_matches('"').to_owned());
            }
        }
        if let Some(value) = trimmed.strip_prefix("dependencies") {
            if let Some(list) = value.trim().strip_prefix('=').map(str::trim) {
                if let Some(inner) = list
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                {
                    for entry in inner.split(',') {
                        let entry = entry.trim().trim_matches('"').trim_matches('\'');
                        let entry = entry
                            .split(['>', '<', '=', ' ', '['])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !entry.is_empty() {
                            dependencies.push(entry.to_owned());
                        }
                    }
                }
            }
        }
    }
    Some((
        name?,
        description.filter(|value| !value.is_empty()),
        dependencies,
    ))
}

/// Read the module line of a `go.mod`.
fn read_go_module(text: &str) -> Option<(String, Option<String>, Vec<String>)> {
    let mut module = None;
    let mut requires = Vec::new();
    let mut in_require = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("module ") {
            let path = rest.trim().trim_matches('"');
            module = Some(path.rsplit('/').next().unwrap_or(path).to_owned());
            continue;
        }
        if trimmed == "require (" {
            in_require = true;
            continue;
        }
        if trimmed == ")" {
            in_require = false;
            continue;
        }
        if in_require || trimmed.starts_with("require ") {
            let entry = trimmed
                .trim_start_matches("require ")
                .split_whitespace()
                .next();
            if let Some(entry) = entry {
                let name = entry.rsplit('/').next().unwrap_or(entry);
                requires.push(name.to_owned());
            }
        }
    }
    Some((module?, None, requires))
}

/// What a structure run would write, without writing it.
/// @param store - pool to compare against
/// @param limit - maximum packages to plan
pub fn plan_structure(store: &Store, limit: Option<usize>) -> Result<StructurePlan> {
    let packages = scan(store.root(), limit);
    let mut upgraded = Vec::new();
    for package in &packages {
        if let Ok(page) = store.read(&package.page_id()) {
            // A page the model wrote is prose a re-run must not throw away.
            if page.front.kind.is_some_and(|kind| kind != Kind::Pointer) {
                upgraded.push(package.name.clone());
            }
        }
    }
    Ok(StructurePlan {
        empty: packages.is_empty(),
        packages: packages
            .iter()
            .map(|package| package.name.clone())
            .collect(),
        upgraded,
    })
}

/// Write one pointer page per package, from files only.
/// @param store - pool to write into
/// @param limit - maximum packages to seed
/// @param run - run tag recorded on every page, so one batch can be removed
pub fn seed_structure(store: &Store, limit: Option<usize>, run: &str) -> Result<StructureReport> {
    let packages = scan(store.root(), limit);
    let mut report = StructureReport::default();
    for package in packages {
        let id = package.page_id();
        if let Ok(page) = store.read(&id) {
            if page.front.kind.is_some_and(|kind| kind != Kind::Pointer) {
                report.skipped += 1;
                continue;
            }
        }
        let request = SaveRequest {
            id: Some(id.clone()),
            title: format!("Package {}", package.name),
            body: structure_body(store.root(), &package),
            tags: vec![format!("run:{run}"), "seed:structure".to_owned()],
            kind: Some(Kind::Pointer),
            subjects: package.subjects(),
            depends: package.dependencies.clone(),
            supersedes: None,
            source: Some(format!("structure:{run}")),
            anchors: anchor_paths(&package)
                .iter()
                .map(|path| crate::page::Anchor::parse(path))
                .collect(),
            expected_rev: None,
            force: false,
        };
        match store.save(&request) {
            Ok(result) => {
                if result.created {
                    report.written += 1;
                } else {
                    report.updated += 1;
                }
                report.pages.push(result.id);
            }
            Err(_) => report.skipped += 1,
        }
    }
    Ok(report)
}

/// Files a structure page cites.
fn anchor_paths(package: &Package) -> Vec<String> {
    let mut paths = vec![package.manifest.clone()];
    if let Some(readme) = &package.readme {
        paths.push(readme.clone());
    }
    paths
}

/// The body of one structure page: what files say, and nothing inferred.
fn structure_body(root: &Path, package: &Package) -> String {
    let mut body = String::new();
    body.push_str(&format!("# Package {}\n\n", package.name));
    body.push_str(&format!(
        "Directory `{}`, declared in `{}`.\n\n",
        package.directory, package.manifest
    ));
    if let Some(description) = &package.description {
        body.push_str(&format!("{description}\n\n"));
    }
    body.push_str("## Declared dependencies\n\n");
    if package.dependencies.is_empty() {
        body.push_str("None declared.\n\n");
    } else {
        for dependency in &package.dependencies {
            body.push_str(&format!("- {dependency}\n"));
        }
        body.push('\n');
    }
    if let Some(readme) = &package.readme {
        body.push_str(&format!("## From `{readme}`\n\n"));
        match fs::read_to_string(root.join(readme)) {
            Ok(text) => {
                let summary = readme_summary(&text);
                body.push_str(&format!("{summary}\n\n"));
            }
            Err(_) => body.push_str("The README could not be read.\n\n"),
        }
    }
    body.push_str("## Status\n\nThis page is a pointer written from files; it names where to look, not what you will find. Run the page pass to replace it with a summary a session wrote from reading the code.\n");
    body
}

/// First paragraphs of a README, capped.
fn readme_summary(text: &str) -> String {
    let mut summary = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.is_empty() {
            if !summary.is_empty() {
                break;
            }
            continue;
        }
        if !summary.is_empty() {
            summary.push(' ');
        }
        summary.push_str(trimmed);
        if summary.len() >= README_BYTES {
            break;
        }
    }
    if summary.is_empty() {
        "The README carries no prose above its first heading.".to_owned()
    } else {
        summary
    }
}

/// What a priced run would do, and what it would cost.
/// @param store - pool to compare against
/// @param limit - maximum packages to upgrade
/// @param token_estimate - tokens one package is expected to spend
/// @param model - model the run would call
pub fn plan_pages(
    store: &Store,
    limit: Option<usize>,
    token_estimate: u64,
    model: Option<String>,
) -> PagePlan {
    let packages = scan(store.root(), limit);
    let mut upgradeable = 0usize;
    let mut skipped = 0usize;
    for package in &packages {
        match store.read(&package.page_id()) {
            Ok(page) if page.front.kind.is_some_and(|kind| kind == Kind::Pointer) => {
                upgradeable += 1
            }
            Ok(_) => skipped += 1,
            Err(_) => upgradeable += 1,
        }
    }
    let unavailable = if model.is_none() {
        Some(
            "no model configured: pass --model or set WAYFINDER_MODEL, and put the key in WAYFINDER_API_KEY"
                .to_owned(),
        )
    } else if packages.is_empty() {
        Some("no package manifests found under this root".to_owned())
    } else {
        None
    };
    PagePlan {
        packages: upgradeable,
        skipped,
        token_estimate_per_package: token_estimate,
        estimated_tokens: upgradeable as u64 * token_estimate,
        model,
        unavailable,
    }
}

/// Upgrade pointer pages with model-written ones.
/// @param store - pool to write into
/// @param writer - where page bodies come from
/// @param limit - maximum packages to upgrade
/// @param run - run tag recorded on every page
pub fn seed_pages(
    store: &Store,
    writer: &dyn PageWriter,
    limit: Option<usize>,
    run: &str,
) -> Result<PageReport> {
    let packages = scan(store.root(), limit);
    let mut report = PageReport::default();
    for package in packages {
        let id = package.page_id();
        if let Ok(page) = store.read(&id) {
            if page.front.kind.is_some_and(|kind| kind == Kind::Pointer) {
                // A pointer is exactly what this pass replaces.
            } else {
                report.skipped += 1;
                continue;
            }
        }
        let request = PageRequest {
            files: anchor_paths(&package),
            prompt: page_prompt(&package),
            package: package.clone(),
        };
        match writer.write(&request) {
            Ok(answer) => {
                let save = SaveRequest {
                    id: Some(id.clone()),
                    title: format!("Package {}", package.name),
                    body: answer.body,
                    tags: vec![format!("run:{run}"), "seed:pages".to_owned()],
                    kind: Some(Kind::Fact),
                    subjects: package.subjects(),
                    depends: package.dependencies.clone(),
                    supersedes: None,
                    source: Some(format!("pages:{run}")),
                    anchors: anchor_paths(&package)
                        .iter()
                        .map(|path| crate::page::Anchor::parse(path))
                        .collect(),
                    expected_rev: None,
                    force: true,
                };
                match store.save(&save) {
                    Ok(result) => {
                        report.saved += 1;
                        report.tokens_spent += answer.tokens;
                        report.pages.push(result.id);
                    }
                    Err(error) => {
                        report.failed += 1;
                        report.last_failure = Some(error.to_string());
                    }
                }
            }
            Err(error) => {
                report.failed += 1;
                report.last_failure = Some(error.to_string());
            }
        }
    }
    Ok(report)
}

/// The prompt one package's page is written from.
fn page_prompt(package: &Package) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "Write one memory page for the package `{}` in this repository.\n\n",
        package.name
    ));
    prompt.push_str(&format!("Directory: `{}`\n", package.directory));
    prompt.push_str(&format!("Manifest: `{}`\n", package.manifest));
    if let Some(readme) = &package.readme {
        prompt.push_str(&format!("README: `{readme}`\n"));
    }
    if !package.dependencies.is_empty() {
        prompt.push_str(&format!(
            "Declared dependencies: {}\n",
            package.dependencies.join(", ")
        ));
    }
    prompt.push_str(
        "\nRead those files and any source they point at. Then answer with markdown only, using \
the headings `## What it is`, `## How it works`, `## Gotchas`. State only what the files show; \
name the files you relied on. No preamble, no code fences around the whole answer.",
    );
    prompt
}

/// Subject keys one pool already uses, so a caller can check coverage.
/// @param store - pool to inspect
pub fn subjects_in_use(store: &Store) -> Result<BTreeSet<String>> {
    let mut subjects = BTreeSet::new();
    for page in store.pages()? {
        for subject in page.front.subjects {
            subjects.insert(subject);
        }
    }
    Ok(subjects)
}
