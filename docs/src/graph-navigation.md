# Build-pinned graph navigation

The operator CLI and read-only MCP share the same graph reader:

```nu
chaosbox query context --repo canix --depth 1 --max-nodes 3 --max-chars 2000 chaosbox
chaosbox query stats --repo canix --limit 10
chaosbox query community --repo canix --limit 20 community:ID
chaosbox query report --repo canix --out GRAPH_REPORT.md
chaosbox query evidence --repo canix rel:ID
```

MCP exposes the same reads as `context`, `stats`, `community`, `report` and
`evidence`.

Each request pins one published build and returns its build ID, generation
and snapshot IDs. Later publications cannot redirect an in-flight request.

## Context budgets

Context seeds with case-insensitive lexical queries, then expands breadth-first
in both directions over the graph's relationships (`bounded-lexical-bfs-v3`). It
drops terms shorter than three characters and English function words unless
nothing would remain.
"Who calls the hub" seeds on `calls` and `hub`, not every label containing "a".
Name hits weigh double qualified-name/path hits. Returned edges retain their
direction, relationship type and identity. A query accepts at most 2,048 bytes
and 32 distinct terms; depth is 1–6, nodes 1–200 and output characters 256–32,000.

Seed queries fetch at most `max_nodes + 1` rows per term. The reader breaks ties
by qualified name and entity ID before the backend limit, then ranks candidates
by matched terms and ID. Expansion reads at most 2,000 relationship rows per
request, plus saturation probes. TypeDB limits each directed query before
merging both directions. Context never needs a complete graph export.

`truncated` is true when candidate, expansion, node or character limits omit
data. Character trimming removes the lowest-priority nodes and their incident
edges, so the output has no dangling endpoints. These packets always report
`exhaustive = false`; a bounded search cannot establish absence of a
dependency or relationship.

## Analytics (`leiden-v1`)

Schema version 8 stores a bounded `navigation-json` packet with each immutable
graph build. Publication computes it once; readers fetch it without
reprojecting the source graph. It retains up to 200 entries per list and
200 member IDs per community, with explicit omission counts and an 8 MiB cap.
The same build yields the same bytes, with no model, embedding or network access.

- **Communities**: deterministic Leiden (fixed visiting order, lowest-index
  ties, greedy refinement, so every community is internally connected) over
  all published relationships, at resolution γ = 1.0. Each community has a
  label (`dominant area · top symbol hub`, where an area is the first two
  directory segments), `cohesion` (distinct internal pairs over possible
  pairs), internal/boundary edge counts and its top five hubs. `quality` is
  the partition's modularity.
- **God nodes**: definitions and symbols ranked by distinct-neighbor degree,
  with incident inferred relationships counted. Files and modules are
  excluded; `hubs` keeps the unfiltered ranking.
- **Surprising connections**: Graphify's score, every term explained in
  `why`: ambiguous +3 / inferred +2 / extracted +1, code↔documentation +2,
  different area +2, different community +1, peripheral (degree ≤ 2) to hub
  (degree ≥ 5) +1. `contains`, `defines` and `imports` are never surprising,
  and a link is listed only when something besides its evidence class is
  unexpected. Each cites `rel_id`.
- **Questions**: ambiguous edges, bridge symbols whose neighbors span ≥ 3
  communities, god nodes with ≥ 2 inferred relationships, function-like
  symbols with no resolved use (test code excluded), communities of ≥ 5
  members whose boundary carries most of their relationships, and the top
  surprise. Density (`cohesion`) shrinks with size, so it is reported but
  never used to suggest splits. Each names the read that answers it (`tool` plus `args`,
  including `repo`).
- **Evidence classes**: `edge_classes` counts sealed evidence per relation;
  any contradiction (a rejected decision) makes a relation `ambiguous`.

Community IDs bind the build and the complete sorted member set; they do not
imply continuity between builds. Community inspection resolves the bounded
member IDs against the pinned build and returns source locations.

`--resolution` (MCP `resolution`, 0.01–10) recomputes the analytics from the
complete bounded export instead of the stored packet; larger values give
smaller communities. Export fallbacks have no sealed classes, so relations of
builds with decision or resolved relationships count as `unknown` there.
The reader recomputes packets from older algorithms the same way. If the
build exceeds the export cap, it serves the stale packet with a `refresh` hint.

`query report` (MCP `report`) renders the same packet as Markdown, Graphify's
`GRAPH_REPORT.md`, printing every cited ID next to its CLI read and MCP call.

Run the normal database migration before using the new package. Existing
builds without an analytics packet remain readable through the bounded legacy
export path. Large legacy builds need a normal snapshot refresh to publish
their packet; migration alone does not rewrite historical evidence.

The explicit export command retains its 10,000-node and 20,000-edge caps.

## Exports

```nu
chaosbox query export --repo canix                                  # node-link JSON
chaosbox query export --repo canix --format html --out graph.html   # interactive, offline
chaosbox query export --repo canix --format mermaid --query "resolve names"
chaosbox query export --repo canix --format graphml --out canix.graphml
chaosbox query export --repo canix --format cypher --out canix.cypher
chaosbox query export --repo canix --format wiki --out canix-wiki/
chaosbox query export --repo canix --format html --community community:ID
```

Every format renders the bounded export (10,000 nodes / 20,000 edges) in
canonical id order, yielding the same bytes for the same build. `--out`
replaces its target atomically. `--community` (an id from `query stats` at the
same `--resolution`) and `--query` (bounded lexical context, depth 2, 200
nodes) narrow any format.

- **json**: the reader's node-link export, now with `source_location` and the
  relationship `scope` (`file` / `cross_file`) like the `run` export.
- **html**: one self-contained file with no network access
  (`default-src 'none'`): force layout, community colors, search, relationship
  type filters (`contains`/`defines` hidden initially) and a panel with
  `file:line` and clickable relationships. Labels are embedded as escaped JSON
  and rendered with `textContent` only. Above 1,000 nodes it shows the 300
  largest multi-member communities as single nodes instead of refusing.
- **mermaid**: a fenced `flowchart` that GitHub, Forgejo and Obsidian render
  natively, showing non-plumbing relationships; above 150 nodes it collapses
  to communities.
- **graphml**: for Gephi, yEd and Cytoscape, with community index and label.
- **cypher**: idempotent `MERGE` statements for Neo4j (`cypher-shell <
  canix.cypher`), relationship types from the closed vocabulary.
- **wiki**: `index.md` plus one note per multi-member community. Plain Markdown
  links work in an Obsidian vault and on any forge. The index lists single-node
  communities.

## MCP answer shape

MCP returns every tool in one `tools/list` page (never `nextCursor`). Each
result obeys `max_tokens` (default 8000, maximum 20000, four characters per
token). When trimming drops whole items, it identifies the affected counts:
`truncated: {shown,total}` for one collection or
`{"<collection>": {shown, total}}` for several. Pre-trim totals survive later
budget passes. A trimmed `neighbors` page returns a `next_cursor` that resumes
at the first omitted row. If whole-item trimming cannot fit the budget, return
the small tool result
`{"error": "result exceeds max_tokens", "max_tokens": N, "needed_tokens": M,
"hint": ...}` rather than an oversized body.

`context`, `lookup` and `explain` accept `include=source|signatures|none`
(default `source`): verified working-tree text for the entity span,
line-numbered (`cat -n` style), in a top-level `source` array. Files whose
recorded snapshot digest no longer matches are pointers marked `stale: true`.
A top-level `freshness` object names served files that are uncommitted or
stale. `review` maps the base side of a `git diff -U0` against the pinned
build's spans (an insertion maps its insertion point), keeps every affected
sibling entity, ranks changed entities by incoming callers, lists test-file
callers separately, reports changes that no indexed span covers under
`unmapped`, and lists a file whose bytes at `base` no longer match the build's
digest once with `"reason": "stale"` instead of asserting entities for it.
