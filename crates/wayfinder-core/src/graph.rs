//! The knowledge graph: pages, the subjects and paths they declare, and the
//! relations between them, laid out deterministically and rendered in the
//! formats a human or another tool reads.

use crate::error::Result;
use crate::page::Kind;
use crate::store::Store;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What one node stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// A memory page.
    Page,
    /// A subject a page declares.
    Subject,
    /// A file a page cites.
    Path,
}

/// One node of the graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphNode {
    /// Stable node id: `page:<id>`, `subject:<key>`, or `path:<path>`.
    pub id: String,
    /// What the node stands for.
    pub kind: NodeKind,
    /// Label the picture draws.
    pub label: String,
    /// Page kind, when the node is a page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_kind: Option<Kind>,
    /// Last update stamp, when the node is a page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// Anchor verdict summary, when the node is a page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<String>,
    /// Pages this node aggregates when a bound hid them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Tags the page declares, for the viewer's detail panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Opening lines of the page body, for the viewer's detail panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<Vec<String>>,
    /// Whether another page in this graph declares that it replaces this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded: Option<bool>,
    /// Page ids a cluster node stands for, so the viewer can name them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Vec<String>>,
}

impl GraphNode {
    /// A node that carries nothing beyond its face.
    fn plain(id: String, kind: NodeKind, label: String) -> Self {
        Self {
            id,
            kind,
            label,
            page_kind: None,
            updated: None,
            freshness: None,
            count: None,
            tags: None,
            excerpt: None,
            superseded: None,
            hidden: None,
        }
    }
}

/// One edge of the graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// Relation the source declares.
    pub predicate: String,
}

/// A whole graph, ready to render.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphView {
    /// Root the pool belongs to.
    pub root: String,
    /// Nodes in id order.
    pub nodes: Vec<GraphNode>,
    /// Edges whose endpoints are both present.
    pub edges: Vec<GraphEdge>,
    /// Whether a bound hid part of the pool.
    pub truncated: bool,
}

/// Rendering the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    /// The wire form other tools read.
    Json,
    /// Graphviz.
    Dot,
    /// A self-contained SVG picture.
    Svg,
    /// A self-contained HTML page.
    Html,
    /// A Mermaid flowchart, for markdown that renders it.
    Mermaid,
}

impl GraphFormat {
    /// Parse one `--format` value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "dot" => Some(Self::Dot),
            "svg" => Some(Self::Svg),
            "html" => Some(Self::Html),
            "mermaid" | "mmd" => Some(Self::Mermaid),
            _ => None,
        }
    }
}

/// Bounds one graph obeys, so a large pool still draws.
#[derive(Debug, Clone, Copy)]
pub struct GraphBounds {
    /// Maximum page nodes.
    pub pages: usize,
    /// Maximum nodes in total.
    pub nodes: usize,
    /// Maximum edges.
    pub edges: usize,
}

impl Default for GraphBounds {
    fn default() -> Self {
        Self {
            pages: 200,
            nodes: 200,
            edges: 400,
        }
    }
}

/// Build the graph of one pool.
/// @param store - the pool to draw
/// @param bounds - ceilings the drawing obeys
/// @param subject - draw only this subject's neighbourhood
pub fn build_graph(store: &Store, bounds: GraphBounds, subject: Option<&str>) -> Result<GraphView> {
    let pages = store.pages()?;
    let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: std::collections::BTreeSet<(String, String, String)> = Default::default();
    let mut truncated = false;
    let mut drawn = 0usize;

    let mut keep: Vec<&crate::page::Page> = pages.iter().collect();
    if let Some(subject) = subject {
        let wanted = crate::page::normalize_subject(subject);
        keep.retain(|page| {
            page.front.subjects.contains(&wanted)
                || page
                    .front
                    .anchors
                    .iter()
                    .any(|anchor| format!("path:{}", anchor.path) == wanted)
        });
    }

    for page in keep {
        let id = format!("page:{}", page.id);
        let check = crate::anchors::check_page(store.root(), page).ok();
        let freshness = check.map(|report| {
            if report.verdicts.is_empty() {
                "unverified".to_owned()
            } else if report.all_fresh {
                "fresh".to_owned()
            } else if report.drifted() > 0 {
                "changed".to_owned()
            } else {
                "unverified".to_owned()
            }
        });
        let superseded = pages
            .iter()
            .any(|other| other.front.supersedes.as_deref() == Some(page.id.as_str()));
        if drawn < bounds.pages && nodes.len() < bounds.nodes {
            nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Page,
                    label: page.front.title.clone(),
                    page_kind: page.front.kind,
                    updated: page.front.updated.clone(),
                    freshness,
                    count: None,
                    tags: (!page.front.tags.is_empty()).then(|| page.front.tags.clone()),
                    excerpt: Some(excerpt_of(&page.body)),
                    superseded: superseded.then_some(true),
                    hidden: None,
                },
            );
            drawn += 1;
        } else {
            // The page budget is spent: the page joins the cluster its lowest
            // subject hub carries, so the drawing still says it exists.
            truncated = true;
            let hub = page
                .front
                .subjects
                .iter()
                .map(|subject| format!("subject:{subject}"))
                .min()
                .unwrap_or_else(|| "cluster:pages".to_owned());
            let hub_id = if nodes.contains_key(&hub) {
                hub
            } else {
                "cluster:pages".to_owned()
            };
            let entry = nodes.entry(hub_id.clone()).or_insert_with(|| {
                GraphNode::plain(hub_id.clone(), NodeKind::Subject, "hidden pages".to_owned())
            });
            entry.count = Some(entry.count.unwrap_or(0) + 1);
            let hidden = entry.hidden.get_or_insert_with(Vec::new);
            if hidden.len() < 50 {
                hidden.push(page.id.clone());
            }
        }
        for subject in &page.front.subjects {
            push_edge(
                &mut edges,
                &mut seen_edges,
                bounds,
                &id,
                &format!("subject:{subject}"),
                "about",
                &mut truncated,
            );
            nodes
                .entry(format!("subject:{subject}"))
                .or_insert_with(|| {
                    GraphNode::plain(
                        format!("subject:{subject}"),
                        NodeKind::Subject,
                        subject.clone(),
                    )
                });
        }
        for anchor in &page.front.anchors {
            let path_id = format!("path:{}", anchor.path);
            push_edge(
                &mut edges,
                &mut seen_edges,
                bounds,
                &id,
                &path_id,
                "cites",
                &mut truncated,
            );
            nodes
                .entry(path_id.clone())
                .or_insert_with(|| GraphNode::plain(path_id, NodeKind::Path, anchor.path.clone()));
        }
        for dependency in &page.front.depends {
            // A dependency is the same node a page declaring that package as a
            // subject draws, so the two edges meet instead of forking the graph.
            let target = format!("subject:package:{dependency}");
            push_edge(
                &mut edges,
                &mut seen_edges,
                bounds,
                &id,
                &target,
                "depends",
                &mut truncated,
            );
            nodes.entry(target.clone()).or_insert_with(|| {
                GraphNode::plain(target, NodeKind::Subject, format!("package:{dependency}"))
            });
        }
        if let Some(supersedes) = &page.front.supersedes {
            push_edge(
                &mut edges,
                &mut seen_edges,
                bounds,
                &id,
                &format!("page:{supersedes}"),
                "supersedes",
                &mut truncated,
            );
        }
    }

    let present: std::collections::BTreeSet<String> = nodes.keys().cloned().collect();
    edges.retain(|edge| present.contains(&edge.from) && present.contains(&edge.to));
    let mut nodes: Vec<GraphNode> = nodes.into_values().collect();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.predicate.cmp(&right.predicate))
            .then_with(|| left.to.cmp(&right.to))
    });
    Ok(GraphView {
        root: store.root().display().to_string(),
        nodes,
        edges,
        truncated,
    })
}

/// Record one edge when both bounds allow it.
fn push_edge(
    edges: &mut Vec<GraphEdge>,
    seen: &mut std::collections::BTreeSet<(String, String, String)>,
    bounds: GraphBounds,
    from: &str,
    to: &str,
    predicate: &str,
    truncated: &mut bool,
) {
    let key = (from.to_owned(), to.to_owned(), predicate.to_owned());
    if !seen.insert(key) {
        return;
    }
    if edges.len() >= bounds.edges {
        *truncated = true;
        return;
    }
    edges.push(GraphEdge {
        from: from.to_owned(),
        to: to.to_owned(),
        predicate: predicate.to_owned(),
    });
}

/// Placement of one node inside the drawing box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

/// Radial placement: page groups fan out by subject hub, everything else on the
/// outer ring. Only node ids seed positions, so the same pool always draws the
/// same picture.
/// @param view - the graph to place
/// @returns the drawing box and one point per node
pub fn layout(view: &GraphView) -> (f64, f64, BTreeMap<String, Point>) {
    const WIDTH: f64 = 900.0;
    const HEIGHT: f64 = 620.0;
    const HUB_RADIUS: f64 = 150.0;
    const PAGE_RADIUS: f64 = 260.0;
    let centre = Point {
        x: WIDTH / 2.0,
        y: HEIGHT / 2.0,
    };
    let mut points: BTreeMap<String, Point> = BTreeMap::new();
    let mut hubs: BTreeMap<String, Vec<&GraphNode>> = BTreeMap::new();
    let mut loose: Vec<&GraphNode> = Vec::new();
    for node in &view.nodes {
        if node.kind == NodeKind::Page {
            // A page joins the lowest subject it declares, so one pool keeps one
            // shape whatever order the edges arrive in.
            let hub = view
                .edges
                .iter()
                .filter(|edge| edge.from == node.id && edge.predicate == "about")
                .map(|edge| edge.to.clone())
                .min();
            match hub {
                Some(hub) => hubs.entry(hub).or_default().push(node),
                None => loose.push(node),
            }
        }
    }
    let total: usize = hubs
        .values()
        .map(|members| members.len().max(1))
        .sum::<usize>()
        + loose.len().max(1);
    let mut cursor = -std::f64::consts::FRAC_PI_2;
    for (hub, members) in &hubs {
        let span = (members.len().max(1) as f64 / total as f64) * std::f64::consts::TAU;
        points.insert(hub.clone(), polar(centre, HUB_RADIUS, cursor + span / 2.0));
        for (index, node) in members.iter().enumerate() {
            let at = cursor + span * ((index as f64 + 0.5) / members.len().max(1) as f64);
            points.insert(node.id.clone(), polar(centre, PAGE_RADIUS, at));
        }
        cursor += span;
    }
    let loose_span = (loose.len().max(1) as f64 / total as f64) * std::f64::consts::TAU;
    for (index, node) in loose.iter().enumerate() {
        let at = cursor + loose_span * ((index as f64 + 0.5) / loose.len().max(1) as f64);
        points.insert(node.id.clone(), polar(centre, PAGE_RADIUS, at));
    }
    cursor += loose_span;
    for node in &view.nodes {
        if node.kind == NodeKind::Page || points.contains_key(&node.id) {
            continue;
        }
        let at = cursor;
        cursor += 0.12;
        points.insert(node.id.clone(), polar(centre, HUB_RADIUS * 0.75, at));
    }
    (WIDTH, HEIGHT, points)
}

/// One point on a circle around the centre.
fn polar(centre: Point, radius: f64, angle: f64) -> Point {
    Point {
        x: round(centre.x + radius * angle.cos()),
        y: round(centre.y + radius * angle.sin()),
    }
}

/// Two decimals keep the emitted coordinates stable.
fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Render a graph in the requested format.
/// @param view - graph to render
/// @param format - wire, picture, or diagram
pub fn render_graph(view: &GraphView, format: GraphFormat) -> Result<String> {
    Ok(match format {
        GraphFormat::Json => serde_json::to_string_pretty(view).unwrap_or_else(|_| "{}".to_owned()),
        GraphFormat::Dot => render_dot(view),
        GraphFormat::Svg => render_svg(view),
        GraphFormat::Html => crate::viewer::render_viewer_html(view),
        GraphFormat::Mermaid => render_mermaid(view),
    })
}

/// Graphviz rendering; `dot -Tsvg` turns it into a picture.
fn render_dot(view: &GraphView) -> String {
    let mut out = String::from(
        "digraph wayfinder {\n  rankdir=LR;\n  node [shape=box, style=rounded, fontname=\"Helvetica\"];\n",
    );
    for node in &view.nodes {
        let shape = match node.kind {
            NodeKind::Page => "box",
            NodeKind::Subject => "ellipse",
            NodeKind::Path => "note",
        };
        let _ = writeln!(
            out,
            "  \"{}\" [label=\"{}\", shape={shape}];",
            node.id,
            escape_dot(&node.label)
        );
    }
    for edge in &view.edges {
        let _ = writeln!(
            out,
            "  \"{}\" -> \"{}\" [label=\"{}\"];",
            edge.from, edge.to, edge.predicate
        );
    }
    out.push_str("}\n");
    out
}

/// Escape one Graphviz label.
fn escape_dot(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

/// A self-contained SVG: no fonts to fetch, no script to run.
fn render_svg(view: &GraphView) -> String {
    let (width, height, points) = layout(view);
    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.0} {height:.0}\" width=\"{width:.0}\" height=\"{height:.0}\" font-family=\"-apple-system, Segoe UI, Helvetica, sans-serif\">\n\
         <rect width=\"100%\" height=\"100%\" fill=\"#0f1115\"/>\n"
    );
    for edge in &view.edges {
        if let (Some(from), Some(to)) = (points.get(&edge.from), points.get(&edge.to)) {
            let _ = writeln!(
                out,
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"#2c3340\" stroke-width=\"1\"/>",
                from.x, from.y, to.x, to.y
            );
        }
    }
    for node in &view.nodes {
        let Some(point) = points.get(&node.id) else {
            continue;
        };
        let (radius, fill, text) = match node.kind {
            NodeKind::Page => (
                if node.page_kind.map(Kind::is_substantive).unwrap_or(false) {
                    9.0
                } else {
                    6.0
                },
                match node.freshness.as_deref() {
                    Some("changed") => "#e0a33a",
                    Some("fresh") => "#4fbf7b",
                    _ => "#5b8def",
                },
                node.label.clone(),
            ),
            NodeKind::Subject => (7.0, "#9aa4b2", node.label.clone()),
            NodeKind::Path => (5.0, "#6b7280", node.label.clone()),
        };
        let _ = writeln!(
            out,
            "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"{radius:.1}\" fill=\"{fill}\"><title>{}</title></circle>",
            point.x,
            point.y,
            escape_xml(&node.label)
        );
        let _ = writeln!(
            out,
            "<text x=\"{:.2}\" y=\"{:.2}\" fill=\"#c8d0dc\" font-size=\"10\" text-anchor=\"middle\">{}</text>",
            point.x,
            point.y + radius + 12.0,
            escape_xml(&short(&text, 28))
        );
    }
    let _ = write!(
        out,
        "<text x=\"16\" y=\"24\" fill=\"#8b95a5\" font-size=\"12\">wayfinder · {} pages · {} edges{}</text>\n</svg>\n",
        view.nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Page)
            .count(),
        view.edges.len(),
        if view.truncated { " · truncated" } else { "" },
    );
    out
}

/// Mermaid flowchart, which renders inside markdown a harness displays.
fn render_mermaid(view: &GraphView) -> String {
    let mut out = String::from("flowchart LR\n");
    for node in &view.nodes {
        let text = escape_mermaid(&short(&node.label, 40));
        match node.kind {
            NodeKind::Page => {
                let _ = writeln!(out, "  {}[\"{text}\"]", mermaid_id(&node.id));
            }
            NodeKind::Subject => {
                let _ = writeln!(out, "  {}(\"{text}\")", mermaid_id(&node.id));
            }
            NodeKind::Path => {
                let _ = writeln!(out, "  {}[/\"{text}\"/]", mermaid_id(&node.id));
            }
        }
    }
    for edge in &view.edges {
        let _ = writeln!(
            out,
            "  {} -->|{}| {}",
            mermaid_id(&edge.from),
            escape_mermaid(&edge.predicate),
            mermaid_id(&edge.to)
        );
    }
    out
}

/// Mermaid node ids must be plain words.
fn mermaid_id(id: &str) -> String {
    id.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

/// Escape one Mermaid label.
fn escape_mermaid(value: &str) -> String {
    value.replace('"', "'").replace('\n', " ")
}

/// Escape text for XML or HTML.
fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Opening lines of one page, for the viewer's detail panel.
fn excerpt_of(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .take(6)
        .map(|line| short(line, 120))
        .collect()
}

/// Trim a label to a bounded width.
fn short(value: &str, width: usize) -> String {
    let value = value.trim();
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}
