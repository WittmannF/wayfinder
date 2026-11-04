//! The browser viewer: one self-contained HTML file that draws the pool and
//! lets a reader click through it.
//!
//! No CDN, no build step, no network: the picture, the data, and the script all
//! live in one file, so it opens from disk on a machine with no connectivity.
//! The layout is computed here, in Rust, and embedded — the script draws and
//! handles selection, filtering, and search; it never lays anything out.

use crate::graph::{GraphView, NodeKind, layout};
use serde_json::json;

/// Render one pool as an interactive page.
/// @param view - the graph to draw
/// @returns a complete HTML document
pub fn render_viewer_html(view: &GraphView) -> String {
    let (width, height, points) = layout(view);
    let payload = json!({
        "root": view.root,
        "truncated": view.truncated,
        "width": width,
        "height": height,
        "nodes": view.nodes.iter().map(|node| json!({
            "id": node.id,
            "kind": match node.kind {
                NodeKind::Page => "page",
                NodeKind::Subject => "subject",
                NodeKind::Path => "path",
            },
            "label": node.label,
            "pageKind": node.page_kind.map(|kind| kind.as_str()),
            "updated": node.updated,
            "freshness": node.freshness,
            "count": node.count,
            "tags": node.tags,
            "excerpt": node.excerpt,
            "superseded": node.superseded,
            "hidden": node.hidden,
            "point": points.get(&node.id).map(|point| json!([point.x, point.y])),
        })).collect::<Vec<_>>(),
        "edges": view.edges,
        "subjects": view.nodes.iter()
            .filter(|node| node.kind == NodeKind::Subject)
            .map(|node| json!({ "id": node.id, "label": node.label }))
            .collect::<Vec<_>>(),
    });
    // A `</script>` inside a label would close the data block early.
    let data = payload.to_string().replace("</", "<\\/");
    let pages = view
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Page)
        .count();
    let changed = view
        .nodes
        .iter()
        .filter(|node| node.freshness.as_deref() == Some("changed"))
        .count();
    let drift = match changed {
        0 => String::new(),
        1 => "<p class=\"drift\" role=\"status\">1 page cites a file that changed — run <code>wayfinder check</code> before trusting it.</p>".to_owned(),
        n => format!(
            "<p class=\"drift\" role=\"status\">{n} pages cite a file that changed — run <code>wayfinder check</code> before trusting them.</p>"
        ),
    };
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>wayfinder — {pages} pages</title>\n<style>{CSS}</style></head>\n<body>\n\
<header><h1>wayfinder knowledge graph</h1><p>{pages} pages · {edges} edges · {root}</p></header>\n\
{drift}\n<div class=\"shell\">\n<main>\n<div class=\"toolbar\">\n\
<input id=\"query\" type=\"search\" placeholder=\"Filter by title, tag, or excerpt\" aria-label=\"Filter nodes\">\n\
<div role=\"group\" aria-label=\"Node kind\" id=\"kinds\"></div>\n\
<label class=\"subject\">Subject <select id=\"subject\" aria-label=\"Subject\"><option value=\"\">All subjects</option></select></label>\n\
<span id=\"count\" role=\"status\" aria-live=\"polite\"></span>\n</div>\n\
<div class=\"canvas\"><svg id=\"canvas\" viewBox=\"0 0 {width:.0} {height:.0}\" role=\"group\" aria-label=\"Pool graph\"></svg></div>\n\
<div class=\"legend\"><span><i class=\"fresh\"></i>anchors match</span><span><i class=\"changed\"></i>cited file changed</span><span><i class=\"unverified\"></i>nothing cited</span></div>\n\
</main>\n<aside id=\"detail\" role=\"region\" aria-label=\"Selected node\" aria-live=\"polite\"><p class=\"empty\">Select a node to read it. Enter or click selects; Escape clears.</p></aside>\n\
</div>\n<script type=\"application/json\" id=\"wayfinder-graph\">{data}</script>\n<script>{SCRIPT}</script>\n</body></html>\n",
        edges = view.edges.len(),
        root = escape(&view.root),
    )
}

/// Escape text for HTML.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Page styles: no external fonts, no framework.
const CSS: &str = "\
:root{color-scheme:dark}*{box-sizing:border-box}\
body{margin:0;background:#0f1115;color:#c8d0dc;font:13px/1.5 -apple-system,'Segoe UI',Helvetica,sans-serif}\
header{padding:14px 18px;border-bottom:1px solid #1d232e}h1{margin:0;font-size:15px;font-weight:600}\
header p{margin:2px 0 0;color:#8b95a5;font-size:12px}\
.drift{margin:0;padding:10px 18px;background:#2a2113;color:#e0a33a;border-bottom:1px solid #3a2f1a;font-size:12px}\
.drift code{background:#00000040;padding:1px 4px;border-radius:4px}\
.shell{display:flex;min-height:calc(100vh - 60px)}\
main{flex:1;min-width:0;display:flex;flex-direction:column}\
.toolbar{display:flex;gap:10px;align-items:center;padding:10px 18px;border-bottom:1px solid #1d232e;flex-wrap:wrap}\
.toolbar input[type=search]{flex:1;min-width:180px;background:#161b23;border:1px solid #263041;color:#c8d0dc;border-radius:6px;padding:6px 8px;font:inherit}\
#kinds{display:flex;gap:4px}\
#kinds button{background:#161b23;border:1px solid #263041;color:#9aa4b2;border-radius:6px;padding:5px 9px;font:inherit;cursor:pointer}\
#kinds button[aria-pressed=true]{background:#1f2a3a;color:#e6edf7;border-color:#3b4c66}\
.subject{display:flex;gap:6px;align-items:center;color:#8b95a5}\
select{background:#161b23;border:1px solid #263041;color:#c8d0dc;border-radius:6px;padding:5px 6px;font:inherit;max-width:220px}\
#count{color:#8b95a5;margin-left:auto}\
.canvas{flex:1;display:grid;place-items:center;padding:10px;overflow:auto}\
svg{max-width:100%;height:auto}\
.node{cursor:pointer;outline:none}\
.node:focus-visible circle{stroke:#7aa2f7;stroke-width:2}\
.node[aria-pressed=true] circle{stroke:#e6edf7;stroke-width:2.5}\
.node text{fill:#c8d0dc;font-size:10px;text-anchor:middle;pointer-events:none}\
.node:hover text{fill:#fff}\
.legend{display:flex;gap:16px;padding:8px 18px;border-top:1px solid #1d232e;color:#8b95a5;font-size:11px}\
.legend i{display:inline-block;width:9px;height:9px;border-radius:50%;margin-right:5px}\
.legend i.fresh{background:#4fbf7b}.legend i.changed{background:#e0a33a}.legend i.unverified{background:#5b8def}\
aside{width:320px;border-left:1px solid #1d232e;padding:16px 18px;overflow:auto}\
aside .empty{color:#6b7280;font-size:12px}\
aside h2{margin:0 0 4px;font-size:14px;font-weight:600;color:#e6edf7}\
aside .sub{color:#8b95a5;font-size:11px;margin:0 0 10px;overflow-wrap:anywhere}\
aside dl{display:grid;grid-template-columns:auto 1fr;gap:4px 10px;margin:0 0 10px;font-size:12px}\
aside dt{color:#6b7280}aside dd{margin:0;color:#c8d0dc;overflow-wrap:anywhere}\
aside .tag{display:inline-block;background:#161b23;border:1px solid #263041;border-radius:999px;padding:1px 7px;margin:0 4px 4px 0;font-size:11px}\
aside pre{margin:0 0 10px;padding:8px;background:#12161d;border-radius:6px;font-size:11px;color:#9aa4b2;white-space:pre-wrap;word-break:break-word;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}\
aside .warn{color:#8b95a5;font-size:11px}aside .warn code{color:#c8d0dc}\
";

/// Viewer script. Draws the embedded layout; owns selection, kind and subject
/// filters, and the text filter.
const SCRIPT: &str = r#"
(function () {
  var data = JSON.parse(document.getElementById('wayfinder-graph').textContent);
  var svg = document.getElementById('canvas');
  var panel = document.getElementById('detail');
  var countEl = document.getElementById('count');
  var NS = 'http://www.w3.org/2000/svg';
  var COLORS = { fresh: '#4fbf7b', changed: '#e0a33a', unverified: '#5b8def', subject: '#9aa4b2', path: '#5a6472', cluster: '#b06fe0' };
  var state = { kind: 'all', subject: '', query: '', selected: null };
  var byId = new Map();
  data.nodes.forEach(function (node) { byId.set(node.id, node); });
  var hubsOf = new Map();
  data.edges.forEach(function (edge) {
    var target = byId.get(edge.to);
    if (!target || target.kind !== 'subject') return;
    var list = hubsOf.get(edge.from) || [];
    if (list.indexOf(edge.to) < 0) list.push(edge.to);
    hubsOf.set(edge.from, list);
  });

  function el(name, attrs) {
    var node = document.createElementNS(NS, name);
    for (var key in attrs) node.setAttribute(key, attrs[key]);
    return node;
  }
  function esc(value) {
    return String(value).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }
  function short(value, limit) {
    value = String(value);
    return value.length > limit ? value.slice(0, limit - 1) + '…' : value;
  }
  function haystack(node) {
    return [node.label, node.pageKind || '', (node.tags || []).join(' '), (node.excerpt || []).join(' '), node.id]
      .join(' ').toLowerCase();
  }
  function radius(node) {
    if (node.count) return 11;
    if (node.kind === 'page') return (node.pageKind && node.pageKind !== 'pointer') ? 9 : 6;
    return node.kind === 'subject' ? 7 : 5;
  }
  function fill(node) {
    if (node.kind === 'page') return COLORS[node.freshness || 'unverified'];
    if (node.count) return COLORS.cluster;
    return COLORS[node.kind] || '#5a6472';
  }
  function accessibleName(node) {
    var bits = [node.label, node.kind];
    if (node.freshness) bits.push(node.freshness);
    if (node.count) bits.push(node.count + ' pages');
    return bits.join(', ');
  }

  function visible() {
    var query = state.query.trim().toLowerCase();
    var keep = new Set();
    data.nodes.forEach(function (node) {
      if (state.kind !== 'all' && node.kind !== state.kind) return;
      if (state.subject) {
        var hub = node.kind === 'subject' && node.id === state.subject;
        var attached = (hubsOf.get(node.id) || []).indexOf(state.subject) >= 0;
        if (!hub && !attached) return;
      }
      if (query && haystack(node).indexOf(query) < 0) return;
      keep.add(node.id);
    });
    if (state.selected && !keep.has(state.selected)) state.selected = null;
    return keep;
  }

  function draw() {
    var keep = visible();
    while (svg.firstChild) svg.removeChild(svg.firstChild);
    data.edges.forEach(function (edge) {
      if (!keep.has(edge.from) || !keep.has(edge.to)) return;
      var from = byId.get(edge.from), to = byId.get(edge.to);
      if (!from.point || !to.point) return;
      svg.appendChild(el('line', {
        x1: from.point[0], y1: from.point[1], x2: to.point[0], y2: to.point[1],
        stroke: '#2c3340', 'stroke-width': 1
      }));
    });
    data.nodes.forEach(function (node) {
      if (!keep.has(node.id) || !node.point) return;
      var group = el('g', {
        'class': 'node', role: 'button', tabindex: '0', 'data-id': node.id,
        'aria-pressed': String(state.selected === node.id),
        'aria-label': accessibleName(node)
      });
      group.appendChild(el('circle', {
        cx: node.point[0], cy: node.point[1], r: radius(node), fill: fill(node),
        opacity: node.kind === 'page' ? 1 : 0.85
      }));
      if (node.count) {
        var badge = el('text', { x: node.point[0], y: node.point[1] + 4, fill: '#0f1115' });
        badge.textContent = '+' + node.count;
        group.appendChild(badge);
      }
      var caption = el('text', { x: node.point[0], y: node.point[1] + radius(node) + 12 });
      caption.textContent = short(node.label, 26);
      group.appendChild(caption);
      group.addEventListener('click', function () { pick(node.id); });
      group.addEventListener('keydown', function (event) {
        if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); pick(node.id); }
        else if (event.key === 'Escape') { pick(null); }
      });
      svg.appendChild(group);
    });
    countEl.textContent = keep.size + ' of ' + data.nodes.length + ' nodes';
    renderPanel();
  }

  function pick(id) {
    state.selected = id;
    draw();
  }

  function renderPanel() {
    var node = state.selected ? byId.get(state.selected) : null;
    if (!node) {
      panel.innerHTML = '<p class="empty">Select a node to read it. Enter or click selects; Escape clears.</p>';
      return;
    }
    var facts = [['kind', node.kind]];
    if (node.pageKind) facts.push(['page kind', node.pageKind]);
    if (node.updated) facts.push(['updated', node.updated]);
    if (node.freshness) facts.push(['anchors', node.freshness]);
    if (node.count) facts.push(['cluster', node.count + ' pages hidden by the node budget']);
    if (node.superseded) facts.push(['superseded', 'another page replaces this one']);
    if (node.hidden && node.hidden.length) {
      facts.push(['hidden pages', node.hidden.slice(0, 20).join(', ') + (node.hidden.length > 20 ? ' …' : '')]);
    }
    var html = '<h2>' + esc(node.label) + '</h2><p class="sub">' + esc(node.id) + '</p><dl>';
    facts.forEach(function (fact) {
      html += '<dt>' + esc(fact[0]) + '</dt><dd>' + esc(fact[1]) + '</dd>';
    });
    html += '</dl>';
    if (node.tags && node.tags.length) {
      html += node.tags.map(function (tag) { return '<span class="tag">' + esc(tag) + '</span>'; }).join('');
    }
    if (node.excerpt && node.excerpt.length) {
      html += '<pre>' + esc(node.excerpt.join('\n')) + '</pre>';
    }
    if (node.kind === 'page') {
      var id = String(node.id).replace(/^page:/, '');
      html += '<p class="warn">Read it with <code>wayfinder read ' + esc(id) +
        '</code>; verify the cited files with <code>wayfinder check ' + esc(id) + '</code>.</p>';
    }
    panel.innerHTML = html;
  }

  var kinds = document.getElementById('kinds');
  ['all', 'page', 'subject', 'path'].forEach(function (kind) {
    var button = document.createElement('button');
    button.type = 'button';
    button.textContent = kind;
    button.setAttribute('aria-pressed', String(state.kind === kind));
    button.addEventListener('click', function () {
      state.kind = kind;
      Array.prototype.forEach.call(kinds.children, function (child) {
        child.setAttribute('aria-pressed', String(child.textContent === state.kind));
      });
      draw();
    });
    kinds.appendChild(button);
  });

  var subjectSelect = document.getElementById('subject');
  data.subjects.forEach(function (subject) {
    var option = document.createElement('option');
    option.value = subject.id;
    option.textContent = subject.label;
    subjectSelect.appendChild(option);
  });
  subjectSelect.addEventListener('change', function () {
    state.subject = subjectSelect.value;
    draw();
  });

  document.getElementById('query').addEventListener('input', function (event) {
    state.query = event.target.value;
    draw();
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape') pick(null);
  });
  draw();
})();
"#;
