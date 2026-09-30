// Opt-in contract pilot. The actual plugin and CLI run; only OpenCode's
// registration/session-delivery API is replaced by an in-process test harness.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [pluginRoot, binary, bundle, scratch, id, statement] = process.argv.slice(2);
const query = "context responses historical evidence citations";
const scope = "private:can";
const repo = "canix";
const packet = JSON.parse(execFileSync(binary, [
  "intelligence", "context", bundle, "--scope", scope, "--repo", repo, query,
], { encoding: "utf8" }));
assert.equal(packet.scope, scope);
assert.equal(packet.historical_data_not_instructions, true);
assert.equal(packet.exhaustive, false);
assert.equal(packet.records.length, 1);
assert.equal(packet.records[0].id, id);

const source = readFileSync(join(pluginRoot, "index.ts"), "utf8");
const registration = 'import { Plugin } from "@opencode/plugin";';
assert.equal(source.split(registration).length, 2);
const compiled = stripTypeScriptTypes(source.replace(
  registration, "const Plugin = { define: (plugin) => plugin };",
));
writeFileSync(join(scratch, "plugin.mjs"), compiled);
copyFileSync(join(pluginRoot, "lib.mjs"), join(scratch, "lib.mjs"));
const { default: plugin } = await import(pathToFileURL(join(scratch, "plugin.mjs")).href);

async function setup(chaosboxBin) {
  const tools = new Map();
  const commands = new Map();
  const delivered = [];
  const stored = new Map();
  let contextHook;
  await plugin.setup({
    options: { bundle, scope, repo, chaosboxBin },
    storage: { set: async (key, value) => stored.set(key, value) },
    session: {
      hook: async (name, callback) => {
        assert.equal(name, "context");
        contextHook = callback;
      },
      prompt: async (value) => delivered.push(value),
    },
    tool: { transform: async (callback) => callback({
      namespace: () => {}, add: (tool) => tools.set(tool.name, tool),
    }) },
    command: { transform: async (callback) => callback({
      add: (command) => commands.set(command.name, command),
    }) },
  });
  const event = { sessionID: "pilot-fixture", agent: "fixture", system: [],
    messages: [{ role: "user", text: query }] };
  await contextHook(event);
  const tool = await tools.get("context").execute({ query });
  await commands.get("chaosbox-context").execute({
    sessionID: "pilot-fixture", prompt: { text: query }, delivery: "fixture",
  });
  const status = JSON.parse((await tools.get("status").execute({})).content);
  return { event, tool, delivered, status, injected: stored.get("chaosbox.injected.pilot-fixture") };
}

const current = await setup(binary);
assert.equal(current.event.system.length, 1, JSON.stringify(current));
for (const text of [current.event.system[0].text, current.tool.content, current.delivered[0].text]) {
  assert.ok(text.includes(id));
  assert.ok(text.includes(statement));
  assert.ok(text.includes("historical evidence"));
  assert.ok(text.includes("citations:"));
}
assert.equal(current.status.degraded, false);
assert.deepEqual(current.injected, [id]);

// Simulate a breaking JSON field rename while keeping actual Rust retrieval.
// This shim and the fixture bundle stay inside the test's private temp directory.
const renamed = join(scratch, "renamed-response.mjs");
writeFileSync(renamed, `#!${process.execPath}
import { execFileSync } from "node:child_process";
const packet = JSON.parse(execFileSync(${JSON.stringify(binary)}, process.argv.slice(2), { encoding: "utf8" }));
packet.items = packet.records;
delete packet.records;
console.log(JSON.stringify(packet));
`, { mode: 0o700 });
const changed = await setup(renamed);
assert.equal(changed.event.system.length, 0);
assert.ok(!changed.tool.content.includes(id));
assert.ok(!changed.delivered[0].text.includes(id));
assert.equal(changed.status.degraded, false);
console.log(JSON.stringify({
  cli_records: packet.records.length,
  verified_consumers: ["context hook", "context tool", "chaosbox-context command"],
  renamed_records_field: "all three silently lose records; degraded remains false",
  host: "registration harness; no live OpenCode session",
  admission: "deterministic test receipt; no model-quality claim",
}));
