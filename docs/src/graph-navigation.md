# Build-pinned graph navigation

The operator CLI and read-only MCP share the same graph reader:

```nu
chaosbox query context --repo canix --depth 1 --max-nodes 3 --max-chars 2000 chaosbox
chaosbox query stats --repo canix --limit 10
chaosbox query community --repo canix --limit 20 community:ID
```

Each request pins one published build. Responses carry its build ID,
generation and snapshot IDs; subsequent publication cannot redirect an
in-flight request to another graph.

## Context budgets

Context uses case-insensitive lexical seed queries followed by undirected
breadth-first expansion. Returned edges retain their original direction,
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

## Statistics and connectivity groups

Schema version 8 stores a bounded `navigation-json` packet with each immutable
graph build. Publication computes exact node/edge counts and weak connected
components once. Reads fetch that packet rather than projecting the source
graph again. It retains up to 200 degree-ranked hubs, 200 largest groups and
200 member IDs per group, with explicit omission counts and an 8 MiB byte cap.

Group IDs bind the build and the complete sorted member set. They describe
weak connectivity, not semantic architecture, and do not imply continuity
between builds. Community inspection resolves the requested bounded member
IDs against the pinned build and returns source locations.

Run the normal database migration before using the new package. Existing
builds without an analytics packet remain readable through the bounded legacy
export path. Large legacy builds need a normal snapshot refresh to publish
their packet; migration alone does not rewrite historical evidence.

The explicit export command retains its 10,000-node and 20,000-edge caps.
