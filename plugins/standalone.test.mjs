import assert from "node:assert/strict";
import { cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

test("all adapters load outside an OpenCode dependency tree", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "chaosbox-standalone-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const name of ["compaction", "scratch", "nix", "intelligence"]) {
    const directory = `chaosbox-${name}`;
    await cp(new URL(`./${directory}`, import.meta.url), join(root, directory), { recursive: true });
    const { default: adapter } = await import(pathToFileURL(join(root, directory, "index.ts")));
    assert.equal(adapter.id, directory);
    assert.equal(typeof adapter.setup, "function");
  }
});
