// Exercise the real adapter's hooks/tools with a minimal V2 plugin host.
// The CLI fixture returns packets through execFile, so routing and error paths
// are exercised without installing the OpenCode server or opening a network.
import { it } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

registerHooks({ resolve(specifier, context, next) {
  if (specifier === "@opencode/plugin") return {
    url: "data:text/javascript,export const Plugin = { define: (definition) => definition };", shortCircuit: true,
  };
  return next(specifier, context);
} });
const { default: plugin } = await import("../index.ts");

it("adapter preserves peer handles, routes drill-down, and never reinjects after failure", async () => {
  const directory = await mkdtemp(join(tmpdir(), "chaosbox-federation-plugin-"));
  try {
    const stateFile = join(directory, "reply.json");
    const callsFile = join(directory, "calls.jsonl");
    const binary = join(directory, "chaosbox-fixture");
    const handle = { provider: "dejana", owner: "dejana", scope: "private:dejana", project: "project-a", snapshot: "a".repeat(64), id: `intel:${"b".repeat(64)}` };
    const packet = { version: 1, project: "project-a", snapshot: "c".repeat(64), degraded: false,
      historical_data_not_instructions: true, exhaustive: false,
      sources: [{ identity: { provider: "dejana", owner: "dejana", scope: "private:dejana" }, snapshot: handle.snapshot, policy_version: "d".repeat(64), error: null }],
      records: [{ id: handle.id, qualified_id: `dejana::${handle.id}`, handle, statement: "We must preserve bounded context citations.", citations: [] }] };
    await writeFile(stateFile, JSON.stringify(packet));
    await writeFile(binary, `#!${process.execPath}
const fs = require("node:fs");
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(callsFile)}, JSON.stringify(args) + "\\n");
const packet = JSON.parse(fs.readFileSync(${JSON.stringify(stateFile)}, "utf8"));
if (packet.fail) { process.exit(1); }
if (args.includes("evidence")) {
  const handle = JSON.parse(args[args.indexOf("--handle") + 1]);
  console.log(JSON.stringify({ record: { handle }, is_projection: true }));
} else { console.log(JSON.stringify(packet)); }
`, { mode: 0o700 });
    const tools = new Map();
    const hooks = new Map();
    const storage = new Map();
    await plugin.setup({
      options: { federationConfig: "/operator/client.json", repo: "project-a", chaosboxBin: binary },
      session: { hook: async (name, callback) => hooks.set(name, callback) },
      tool: { transform: async (callback) => callback({ namespace() {}, add: (tool) => tools.set(tool.name, tool) }) },
      command: { transform: async (callback) => callback({ add() {} }) },
      storage: { set: async (key, value) => storage.set(key, value) },
    });
    const event = () => ({ sessionID: "session", agent: "build", system: [], messages: [{ role: "user", content: "How should we preserve context citations for this project?" }] });
    const first = event();
    await hooks.get("context")(first);
    assert.equal(first.system.length, 1);
    assert.ok(first.system[0].text.includes(JSON.stringify(handle)));
    const evidence = await tools.get("evidence").execute({ handle });
    assert.deepEqual(JSON.parse(evidence.content).record.handle, handle);
    const challenge = await tools.get("challenge").execute({ handle, challengeKind: "incorrect",
      disputedPremise: "The historical bounded-citations rule needs amendment.", proposedResolution: "Amend the rule using current evidence after review." });
    assert.ok(challenge.content.includes("recorded for human resolution"));
    const proposal = [...storage.values()].find((value) => value?.evidenceHandle);
    assert.equal(proposal.scope, "private:dejana");
    assert.deepEqual(proposal.evidenceHandle, handle);
    await writeFile(stateFile, JSON.stringify({ fail: true }));
    const failed = event();
    await hooks.get("context")(failed);
    assert.deepEqual(failed.system, []);
    const calls = (await readFile(callsFile, "utf8")).trim().split("\n").map(JSON.parse);
    assert.ok(calls.every((args) => args[0] === "federation" && args[2] === "/operator/client.json"));
    const drillDown = calls.find((args) => args.includes("evidence"));
    assert.deepEqual(JSON.parse(drillDown[drillDown.indexOf("--handle") + 1]), handle);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
