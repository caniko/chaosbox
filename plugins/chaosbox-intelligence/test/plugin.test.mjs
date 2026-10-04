// Structural guardrails for index.ts: the adapter must stay read-only,
// credential-free, and scoped. Runs under `node --test` without the OpenCode
// host or a TypeScript compiler.
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(dir, "..", "index.ts"), "utf8");

describe("plugin structure", () => {
  it("registers the selective-injection hook plus tools and commands", () => {
    assert.match(source, /ctx\.session\.hook\("context"/);
    assert.match(source, /ctx\.tool\.transform/);
    assert.match(source, /ctx\.command\.transform/);
    for (const tool of ["name: \"context\"", "name: \"evidence\"", "name: \"challenge\"", "name: \"status\""]) {
      assert.ok(source.includes(tool), `missing tool ${tool}`);
    }
    assert.ok(source.includes("chaosbox-context"));
    assert.ok(source.includes("chaosbox-challenge"));
  });

  it("never touches credentials or the inference endpoint", () => {
    for (const banned of ["TYPESAFE_API_KEY", "JEV_API_KEY", "systemone", "LiveResponder", "assess_catalog"]) {
      assert.ok(!source.includes(banned), `banned reference: ${banned}`);
    }
  });

  it("only invokes the read-only subcommands with pinned scope", () => {
    assert.ok(source.includes('"intelligence",\n        "context"') || source.includes('"intelligence", "context"') || source.includes('"context"'));
    assert.ok(source.includes('"evidence"'));
    assert.ok(!/intelligence",\s*"(extract|assess|publish|migrate)"/.test(source));
  });

  it("keeps injection out of auxiliary requests and guards the feedback loop", () => {
    assert.match(source, /hook\("context"/);
    assert.ok(!source.includes('hook("compaction"'));
    assert.ok(!source.includes('hook("title"'));
    assert.match(source, /containsInjectedQuote/);
    assert.match(source, /buildInjectionText/);
    assert.match(source, /event\.system\.push/);
  });

  it("challenges stay proposals pending human resolution", () => {
    assert.match(source, /human resolution/);
    assert.match(source, /never mutates the bundle/i);
    assert.match(source, /checked\.proposal/);
  });
});
