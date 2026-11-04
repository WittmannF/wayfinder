//! Seeding: a pool on day one.
//!
//! Two passes with different prices. The structure pass reads manifests,
//! READMEs, entry files, and the shape of the source tree, and writes one
//! pointer page per target; it calls no model. The page pass replaces those
//! pointers with model-written pages, one child per target, behind a plan that
//! states the cost first.
//!
//! A target is a package whose code fits on one page, or, for a package whose
//! directories hold code of their own, one page per such directory. The graph
//! then follows the code instead of the build files, so a repository with a
//! single manifest still describes its layout.

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

/// Source-file extensions the code map counts. Everything else is data, docs, or
/// an artifact, and belongs to no directory page.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "rb", "cs",
    "cpp", "cc", "c", "h", "hpp", "swift", "php", "scala", "ex", "exs", "sh", "sql", "vue",
    "svelte",
];

/// Source files a package holds before the structure pass splits it into one page
/// per code directory. Below this, one page describes the whole package.
const SPLIT_FILE_THRESHOLD: usize = 24;

/// Source files a directory must hold directly before it earns its own page.
const AREA_MIN_FILES: usize = 3;

/// Directory pages one package writes at most, largest first.
const MAX_AREAS: usize = 40;

/// Files one page lists in its body and cites as anchors.
const LISTED_FILES: usize = 12;

/// Entry points one page records.
const MAX_ENTRY_POINTS: usize = 8;

/// Longest page id the pool accepts, repeated from the page-id rule.
const PAGE_ID_MAX: usize = 64;

/// Conventional entry files, probed inside a package directory in this order.
/// Only the ones that exist are recorded, so the list costs a stat per name.
const ENTRY_POINT_CANDIDATES: &[&str] = &[
    "main.py",
    "app.py",
    "__main__.py",
    "manage.py",
    "asgi.py",
    "wsgi.py",
    "server.py",
    "conftest.py",
    "src/main.py",
    "app/main.py",
    "main.rs",
    "lib.rs",
    "src/main.rs",
    "src/lib.rs",
    "main.go",
    "cmd/server/main.go",
    "index.js",
    "index.ts",
    "index.mjs",
    "main.js",
    "main.ts",
    "server.js",
    "server.ts",
    "app.js",
    "app.ts",
    "src/index.js",
    "src/index.ts",
    "src/main.ts",
    "src/server.ts",
];

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

/// One code directory inside a package, large enough to describe on its own page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    /// Directory relative to the root, with `/` separators.
    pub directory: String,
    /// Directory relative to the package directory.
    pub name: String,
    /// Source files the directory holds directly, relative to the root, sorted,
    /// capped at [`LISTED_FILES`].
    pub files: Vec<String>,
    /// How many source files the directory holds directly, listed or not.
    pub file_count: usize,
    /// Entry points inside the directory, relative to the root.
    pub entry_points: Vec<String>,
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
    /// Files a run starts from: the ones the manifest declares, then the
    /// conventional entry files of the language. Relative to the root.
    pub entry_points: Vec<String>,
    /// Source files the directory holds, recursively.
    pub source_files: usize,
    /// Code directories that earn their own page; empty when the package's code
    /// fits on one page.
    pub areas: Vec<Area>,
}

impl Package {
    /// Page id the structure pass writes, and the page pass upgrades.
    pub fn page_id(&self) -> String {
        crate::page::slugify(&format!("package {}", self.name))
    }

    /// Page id of one code directory's page.
    /// @param area - code directory inside this package
    pub fn area_page_id(&self, area: &Area) -> String {
        crate::page::slugify(&format!("{} {}", self.name, area.name))
    }

    /// Subjects the page declares.
    pub fn subjects(&self) -> Vec<String> {
        let mut subjects = vec![format!("package:{}", self.name)];
        if !self.directory.is_empty() && self.directory != "." {
            subjects.push(format!("path:{}", self.directory));
        }
        subjects
    }

    /// Files this package's own page cites.
    pub fn anchors(&self) -> Vec<String> {
        let mut paths = vec![self.manifest.clone()];
        if let Some(readme) = &self.readme {
            paths.push(readme.clone());
        }
        paths.extend(self.entry_points.iter().cloned());
        paths
    }
}

/// One page the structure pass writes: a package's own page, or the page of one
/// code directory inside a package large enough to split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureTarget {
    /// Page id the structure pass writes and the page pass upgrades.
    pub id: String,
    /// Page title.
    pub title: String,
    /// Package the page belongs to.
    pub package: Package,
    /// Code directory this page describes; `None` for the package's own page.
    pub area: Option<Area>,
}

impl StructureTarget {
    /// Subjects the page declares.
    pub fn subjects(&self) -> Vec<String> {
        match &self.area {
            None => self.package.subjects(),
            Some(area) => vec![
                format!("package:{}", self.package.name),
                format!("path:{}", area.directory),
            ],
        }
    }

    /// Graph relations the page records: the runtime dependencies of a package,
    /// or the package page a code directory belongs to.
    pub fn depends(&self) -> Vec<String> {
        match &self.area {
            None => self.package.dependencies.clone(),
            Some(_) => vec![format!("package:{}", self.package.name)],
        }
    }

    /// Files the page cites.
    pub fn anchors(&self) -> Vec<String> {
        match &self.area {
            None => self.package.anchors(),
            Some(area) => area.files.clone(),
        }
    }

    /// Files a model should read to write this page.
    pub fn files(&self) -> Vec<String> {
        match &self.area {
            None => self.package.anchors(),
            Some(area) => area
                .entry_points
                .iter()
                .chain(area.files.iter())
                .cloned()
                .collect(),
        }
    }
}

/// What a structure run would do.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StructurePlan {
    /// Packages found, in walk order.
    pub packages: Vec<String>,
    /// Code directories that earn their own page, as `package / directory`.
    pub areas: Vec<String>,
    /// Page ids whose page already carries a model-written body, which a re-run
    /// leaves alone.
    pub upgraded: Vec<String>,
    /// Pages a run would write: one per package, plus one per code directory.
    pub pages: usize,
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
    /// Pages left alone because they already carry a model-written body.
    pub skipped: usize,
    /// Page ids written, in order.
    pub pages: Vec<String>,
}

/// An estimate the priced pass shows before it spends anything.
#[derive(Debug, Clone, Serialize)]
pub struct PagePlan {
    /// Pages that would be upgraded.
    pub pages: usize,
    /// Pages skipped because the pass already wrote them.
    pub skipped: usize,
    /// Tokens per page the estimate uses.
    pub token_estimate_per_page: u64,
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
    /// Page the model should write.
    pub target: StructureTarget,
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
    /// Write one page body for one target.
    /// @param request - the target, its files, and the prompt
    fn write(&self, request: &PageRequest) -> Result<PageAnswer>;
}

/// What a priced run did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageReport {
    /// Pages upgraded.
    pub saved: usize,
    /// Pages whose page already carried a model-written body.
    pub skipped: usize,
    /// Pages whose writing failed.
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
    let declared = declared_entry_points(kind, &text);
    let (source_files, areas) = code_map(root, directory);
    Some(Package {
        name: name.trim().to_owned(),
        directory: relative_directory,
        manifest: relative(root, manifest),
        entry_points: entry_points_of(root, directory, &declared),
        readme,
        description,
        dependencies,
        source_files,
        areas,
    })
}

/// Count the source files of one package and pick the directories that earn
/// their own page.
///
/// A directory earns a page when it holds [`AREA_MIN_FILES`] source files of its
/// own; the package earns the split when it holds [`SPLIT_FILE_THRESHOLD`] in
/// total, because below that one page describes everything without losing
/// anything. The package's own directory stays on the package page.
/// @param root - project root the paths are relative to
/// @param directory - directory of the package being read
fn code_map(root: &Path, directory: &Path) -> (usize, Vec<Area>) {
    let mut collected = Vec::new();
    let mut total = 0;
    collect_code(root, directory, directory, 0, &mut total, &mut collected);
    if total < SPLIT_FILE_THRESHOLD {
        return (total, Vec::new());
    }
    let mut areas: Vec<Area> = collected
        .into_iter()
        .filter(|(_, name, files)| name != "." && files.len() >= AREA_MIN_FILES)
        .map(|(directory, name, files)| Area {
            entry_points: Vec::new(),
            directory,
            name,
            file_count: files.len(),
            files: files.into_iter().take(LISTED_FILES).collect(),
        })
        .collect();
    // Largest first: a package with more code directories than the cap keeps the
    // ones that hold the code, and the name breaks every tie the same way twice.
    areas.sort_by(|left, right| {
        right
            .file_count
            .cmp(&left.file_count)
            .then_with(|| left.name.cmp(&right.name))
    });
    areas.truncate(MAX_AREAS);
    for area in &mut areas {
        area.entry_points = entry_points_of(root, &root.join(&area.directory), &[]);
    }
    (total, areas)
}

/// Collect every directory under one package with the source files it holds
/// directly, as `(directory, path below the package, files)`.
fn collect_code(
    root: &Path,
    base: &Path,
    directory: &Path,
    depth: usize,
    total: &mut usize,
    out: &mut Vec<(String, String, Vec<String>)>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut files = Vec::new();
    let mut children = Vec::new();
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
        if is_source(name) {
            files.push(relative(root, &path));
        }
    }
    *total += files.len();
    files.sort();
    out.push((relative(root, directory), relative(base, directory), files));
    children.sort();
    for child in children {
        collect_code(root, base, &child, depth + 1, total, out);
    }
}

/// Whether one file name carries a source extension.
fn is_source(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty() && SOURCE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
}

/// Entry files of one directory: the ones its manifest declares, then the
/// conventional names of the language, keeping only files that exist.
fn entry_points_of(root: &Path, directory: &Path, declared: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for candidate in declared
        .iter()
        .map(String::as_str)
        .chain(ENTRY_POINT_CANDIDATES.iter().copied())
    {
        if paths.len() >= MAX_ENTRY_POINTS {
            break;
        }
        let Some(resolved) = resolve_entry(directory, candidate) else {
            continue;
        };
        let spec = relative(root, &resolved);
        if seen.insert(spec.clone()) {
            paths.push(spec);
        }
    }
    paths
}

/// The file one entry-point candidate names, when the directory holds it. A
/// declared path may name a built artifact, so `src/` is tried as well.
fn resolve_entry(directory: &Path, candidate: &str) -> Option<PathBuf> {
    let direct = directory.join(candidate);
    if direct.is_file() {
        return Some(direct);
    }
    let source = directory.join("src").join(candidate);
    source.is_file().then_some(source)
}

/// Entry files a manifest declares, relative to the package directory.
fn declared_entry_points(kind: ManifestKind, text: &str) -> Vec<String> {
    match kind {
        ManifestKind::Node => declared_node_entries(text),
        ManifestKind::Cargo => declared_cargo_entries(text),
        ManifestKind::Python => declared_python_entries(text),
        ManifestKind::Go => Vec::new(),
    }
}

/// `main`, `module`, and the `bin` targets of a `package.json`.
fn declared_node_entries(text: &str) -> Vec<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for key in ["main", "module"] {
        if let Some(value) = document.get(key).and_then(|value| value.as_str()) {
            paths.push(value.to_owned());
        }
    }
    match document.get("bin") {
        Some(serde_json::Value::String(value)) => paths.push(value.clone()),
        Some(serde_json::Value::Object(targets)) => {
            for value in targets.values().filter_map(|value| value.as_str()) {
                paths.push(value.to_owned());
            }
        }
        _ => {}
    }
    paths
}

/// `[[bin]]` paths of a `Cargo.toml`.
fn declared_cargo_entries(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut in_bin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_bin = trimmed == "[[bin]]";
            continue;
        }
        if in_bin
            && let Some(value) = trimmed.strip_prefix("path")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            paths.push(value.trim().trim_matches('"').to_owned());
        }
    }
    paths
}

/// Script targets of a `pyproject.toml`, as the module behind `module:function`.
fn declared_python_entries(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(header) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = header.trim().to_owned();
            continue;
        }
        if section != "project.scripts" && section != "tool.poetry.scripts" {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let module = value
            .trim()
            .trim_matches('"')
            .split(':')
            .next()
            .unwrap_or("");
        let module = module.trim();
        if !module.is_empty() {
            paths.push(format!("{}.py", module.replace('.', "/")));
        }
    }
    paths
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
            if let Some(value) = trimmed.strip_prefix("name")
                && let Some(value) = value.trim_start().strip_prefix('=')
            {
                name = Some(value.trim().trim_matches('"').to_owned());
            }
            if let Some(value) = trimmed.strip_prefix("description")
                && let Some(value) = value.trim_start().strip_prefix('=')
            {
                description = Some(value.trim().trim_matches('"').to_owned());
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
        if let Some(value) = trimmed.strip_prefix("name")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            name = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("description")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            description = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("dependencies")
            && let Some(list) = value.trim().strip_prefix('=').map(str::trim)
            && let Some(inner) = list
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

/// Every page the structure pass writes: one per package, plus one per code
/// directory of a package large enough to split.
/// @param root - project root to walk
/// @param limit - maximum packages to visit
pub fn structure_targets(root: &Path, limit: Option<usize>) -> Vec<StructureTarget> {
    let mut used = BTreeSet::new();
    let mut targets = Vec::new();
    for package in scan(root, limit) {
        targets.push(StructureTarget {
            id: unique_id(package.page_id(), &mut used),
            title: format!("Package {}", package.name),
            area: None,
            package: package.clone(),
        });
        for area in &package.areas {
            targets.push(StructureTarget {
                id: unique_id(package.area_page_id(area), &mut used),
                title: format!("{} / {}", package.name, area.name),
                area: Some(area.clone()),
                package: package.clone(),
            });
        }
    }
    targets
}

/// A page id no earlier target in this run claimed, suffixed when the slug of a
/// long path collides with one already used.
fn unique_id(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut counter = 2usize;
    loop {
        let suffix = format!("-{counter}");
        let keep = PAGE_ID_MAX.saturating_sub(suffix.len());
        let mut candidate = base.chars().take(keep).collect::<String>();
        while candidate.ends_with('-') {
            candidate.pop();
        }
        candidate.push_str(&suffix);
        if used.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

/// What a structure run would write, without writing it.
/// @param store - pool to compare against
/// @param limit - maximum packages to plan
pub fn plan_structure(store: &Store, limit: Option<usize>) -> Result<StructurePlan> {
    let targets = structure_targets(store.root(), limit);
    let mut upgraded = Vec::new();
    let mut packages: Vec<String> = Vec::new();
    let mut areas = Vec::new();
    for target in &targets {
        if let Ok(page) = store.read(&target.id)
            && page.front.kind.is_some_and(|kind| kind != Kind::Pointer)
        {
            // A page the model wrote is prose a re-run must not throw away.
            upgraded.push(target.id.clone());
        }
        if !packages.contains(&target.package.name) {
            packages.push(target.package.name.clone());
        }
        if let Some(area) = &target.area {
            areas.push(format!("{} / {}", target.package.name, area.name));
        }
    }
    Ok(StructurePlan {
        empty: packages.is_empty(),
        pages: targets.len(),
        packages,
        areas,
        upgraded,
    })
}

/// Write one pointer page per target, from files only.
/// @param store - pool to write into
/// @param limit - maximum packages to seed
/// @param run - run tag recorded on every page, so one batch can be removed
pub fn seed_structure(store: &Store, limit: Option<usize>, run: &str) -> Result<StructureReport> {
    let targets = structure_targets(store.root(), limit);
    let mut report = StructureReport::default();
    for target in targets {
        if let Ok(page) = store.read(&target.id)
            && page.front.kind.is_some_and(|kind| kind != Kind::Pointer)
        {
            report.skipped += 1;
            continue;
        }
        let request = SaveRequest {
            id: Some(target.id.clone()),
            title: target.title.clone(),
            body: structure_body(store.root(), &target),
            tags: vec![format!("run:{run}"), "seed:structure".to_owned()],
            kind: Some(Kind::Pointer),
            subjects: target.subjects(),
            depends: target.depends(),
            supersedes: None,
            source: Some(format!("structure:{run}")),
            anchors: target
                .anchors()
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

/// The body of one structure page: what files say, and nothing inferred.
fn structure_body(root: &Path, target: &StructureTarget) -> String {
    match &target.area {
        None => package_body(root, &target.package),
        Some(area) => area_body(&target.package, area),
    }
}

/// The body of one package's own page.
fn package_body(root: &Path, package: &Package) -> String {
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
    if !package.entry_points.is_empty() {
        body.push_str("## Entry points\n\nStart here to follow a change inward.\n\n");
        for entry in &package.entry_points {
            body.push_str(&format!("- `{entry}`\n"));
        }
        body.push('\n');
    }
    if !package.areas.is_empty() {
        body.push_str(&format!(
            "## Code map\n\n{} source files in {} directories.\n\n",
            package.source_files,
            package.areas.len()
        ));
        for area in &package.areas {
            body.push_str(&format!("- `{}` — {} files\n", area.name, area.file_count));
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

/// The body of one code directory's page.
fn area_body(package: &Package, area: &Area) -> String {
    let mut body = String::new();
    body.push_str(&format!("# {} / {}\n\n", package.name, area.name));
    body.push_str(&format!(
        "Directory `{}` inside package `{}`, {} source file(s).\n\n",
        area.directory, package.name, area.file_count
    ));
    if !area.entry_points.is_empty() {
        body.push_str("## Entry points\n\n");
        for entry in &area.entry_points {
            body.push_str(&format!("- `{entry}`\n"));
        }
        body.push('\n');
    }
    body.push_str("## Files\n\n");
    for file in &area.files {
        body.push_str(&format!("- `{file}`\n"));
    }
    if area.file_count > area.files.len() {
        body.push_str(&format!(
            "\n{} more file(s) here, not listed.\n",
            area.file_count - area.files.len()
        ));
    }
    body.push('\n');
    body.push_str("## Status\n\nThis page is a pointer written from files; it names where to look, not what you will find. Run the page pass to replace it with a summary a session wrote from reading the files.\n");
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
/// @param limit - maximum packages to visit
/// @param token_estimate - tokens one page is expected to spend
/// @param model - model the run would call
pub fn plan_pages(
    store: &Store,
    limit: Option<usize>,
    token_estimate: u64,
    model: Option<String>,
) -> PagePlan {
    let targets = structure_targets(store.root(), limit);
    let mut upgradeable = 0usize;
    let mut skipped = 0usize;
    for target in &targets {
        match store.read(&target.id) {
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
    } else if targets.is_empty() {
        Some("no package manifests found under this root".to_owned())
    } else {
        None
    };
    PagePlan {
        pages: upgradeable,
        skipped,
        token_estimate_per_page: token_estimate,
        estimated_tokens: upgradeable as u64 * token_estimate,
        model,
        unavailable,
    }
}

/// Upgrade pointer pages with model-written ones.
/// @param store - pool to write into
/// @param writer - where page bodies come from
/// @param limit - maximum pages to upgrade
/// @param run - run tag recorded on every page
pub fn seed_pages(
    store: &Store,
    writer: &dyn PageWriter,
    limit: Option<usize>,
    run: &str,
) -> Result<PageReport> {
    let targets = structure_targets(store.root(), limit);
    let mut report = PageReport::default();
    for target in targets {
        if let Ok(page) = store.read(&target.id)
            && page.front.kind.is_some_and(|kind| kind != Kind::Pointer)
        {
            report.skipped += 1;
            continue;
        }
        let request = PageRequest {
            files: target.files(),
            prompt: page_prompt(&target),
            target: target.clone(),
        };
        match writer.write(&request) {
            Ok(answer) => {
                let save = SaveRequest {
                    id: Some(target.id.clone()),
                    title: target.title.clone(),
                    body: answer.body,
                    tags: vec![format!("run:{run}"), "seed:pages".to_owned()],
                    kind: Some(Kind::Fact),
                    subjects: target.subjects(),
                    depends: target.depends(),
                    supersedes: None,
                    source: Some(format!("pages:{run}")),
                    anchors: target
                        .anchors()
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

/// The prompt one page is written from.
fn page_prompt(target: &StructureTarget) -> String {
    match &target.area {
        None => package_prompt(&target.package),
        Some(area) => area_prompt(&target.package, area),
    }
}

/// The prompt one package page is written from.
fn package_prompt(package: &Package) -> String {
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
    if !package.entry_points.is_empty() {
        prompt.push_str(&format!(
            "Entry points: {}\n",
            package
                .entry_points
                .iter()
                .map(|entry| format!("`{entry}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !package.dependencies.is_empty() {
        prompt.push_str(&format!(
            "Declared dependencies: {}\n",
            package.dependencies.join(", ")
        ));
    }
    if !package.areas.is_empty() {
        prompt.push_str(&format!(
            "Code directories: {}\n",
            package
                .areas
                .iter()
                .map(|area| format!("`{}` ({} files)", area.name, area.file_count))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    prompt.push_str(PAGE_PROMPT_TAIL);
    prompt
}

/// The prompt one code directory's page is written from.
fn area_prompt(package: &Package, area: &Area) -> String {
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "Write one memory page for the directory `{}` of the package `{}` in this repository.\n\n",
        area.name, package.name
    ));
    prompt.push_str(&format!(
        "Directory: `{}` ({} source files)\n",
        area.directory, area.file_count
    ));
    prompt.push_str(&format!("Package manifest: `{}`\n", package.manifest));
    prompt.push_str("Files to start from:\n");
    for file in area.entry_points.iter().chain(area.files.iter()) {
        prompt.push_str(&format!("- `{file}`\n"));
    }
    prompt.push_str(PAGE_PROMPT_TAIL);
    prompt
}

/// What every page prompt asks for, whatever the page describes.
const PAGE_PROMPT_TAIL: &str = "\nRead those files and any source they point at. Then answer with markdown only, using \
the headings `## What it is`, `## How it works`, `## Gotchas`. State only what the files show; \
name the files you relied on. No preamble, no code fences around the whole answer.";

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
