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

Each request pins one published build. Responses carry its build ID,
generation and snapshot IDs; subsequent publication cannot redirect an
in-flight request to another graph.

## Context budgets

Context uses case-insensitive lexical seed queries followed by undirected
breadth-first expansion (`bounded-lexical-bfs-v3`). Terms shorter than three
characters and English function words are dropped unless nothing would
remain, so "who calls the hub" seeds on `calls` and `hub` rather than every
label containing "a". Name hits weigh double qualified-name/path hits. Returned edges retain their original direction,
relationship type and identity. A query accepts at most 2,048 bytes and 32
distinct terms; depth is 1–6, nodes 1–200 and output characters 256–32,000.

Seed queries fetch at most `max_nodes + 1` rows per term. Ties are ordered by
qualified name and entity ID before the backend limit, then candidates are
ranked by matched terms and ID. Expansion reads at most 2,000 relationship
rows across the request, plus saturation probes. TypeDB applies limits to
each directed query before merging the two directions. No context request
needs a complete graph export.

`truncated` is true when candidate, expansion, node or character limits omit
data. Character trimming removes the lowest-priority nodes and their incident
edges, so the output has no dangling endpoints. These packets always report
`exhaustive = false`; a bounded search cannot establish absence of a
dependency or relationship.

## Analytics (`leiden-v1`)

Schema version 8 stores a bounded `navigation-json` packet with each immutable
graph build. Publication computes it once; reads fetch it rather than
projecting the source graph again. The packet retains up to 200 entries per
list and 200 member IDs per community, with explicit omission counts and an
8 MiB byte cap. Everything below is deterministic: the same build yields the
same bytes, and no model, embedding or network access is involved.

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
Packets from an older algorithm are recomputed the same way; when the build
exceeds the export cap, the stale packet is served with a `refresh` hint.

`query report` (MCP `report`) renders the same packet as Markdown, Graphify's
`GRAPH_REPORT.md`, printing every cited ID next to its CLI read and MCP call.

Run the normal database migration before using the new package. Existing
builds without an analytics packet remain readable through the bounded legacy
export path. Large legacy builds need a normal snapshot refresh to publish
their packet; migration alone does not rewrite historical evidence.

The explicit export command retains its 10,000-node and 20,000-edge caps.
