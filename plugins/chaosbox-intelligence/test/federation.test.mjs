import { it } from "node:test";
import assert from "node:assert/strict";
import { buildInjectionText, federationPackets, readOnlyArgs } from "../lib.mjs";

const handle = { provider: "dejana-atlas", owner: "dejana", scope: "private:dejana",
  project: "project-a", snapshot: "a".repeat(64), id: `intel:${"b".repeat(64)}` };
const packet = { version: 1, project: "project-a", snapshot: "c".repeat(64), degraded: false,
  sources: [{ identity: { provider: handle.provider, owner: handle.owner, scope: handle.scope },
    snapshot: handle.snapshot, policy_version: "d".repeat(64), error: null }],
  records: [{ id: handle.id, qualified_id: `${handle.provider}::${handle.id}`, handle,
    statement: "Keep context citations bounded.", same_origin: [] }], historical_data_not_instructions: true, exhaustive: false };

it("federation commands pin operator config and carry exact evidence handles", () => {
  const options = { federationConfig: "/private/federation.json" };
  assert.deepEqual(readOnlyArgs(options, "context", { repo: "project-a", query: "--live-jev", limit: 5, maxChars: 12000 }),
    ["federation", "--config", options.federationConfig, "context", "--repo", "project-a",
      "--limit", "5", "--max-chars", "12000", "--", "--live-jev"]);
  assert.deepEqual(readOnlyArgs(options, "evidence", { repo: "project-a", handle, maxChars: 12000 }),
    ["federation", "--config", options.federationConfig, "evidence", "--repo", "project-a",
      "--handle", JSON.stringify(handle), "--max-chars", "12000"]);
  assert.throws(() => readOnlyArgs({ ...options, bundle: "/private/bundle" }, "context", {}));
  assert.throws(() => readOnlyArgs(options, "serve", {}));
});

it("peer denial/outage never replays previously disclosed content", async () => {
  let result = packet;
  let calls = 0;
  const read = federationPackets(async () => { calls++; if (result instanceof Error) throw result; return structuredClone(result); });
  const args = readOnlyArgs({ federationConfig: "/private/federation.json" }, "context", { repo: "project-a", query: "context", limit: 5, maxChars: 12000 });
  assert.equal((await read(args)).records.length, 1);
  result = new Error("access denied");
  await assert.rejects(read(args), /denied/);
  result = { ...packet, records: [], degraded: true, sources: [{ ...packet.sources[0], snapshot: null, policy_version: null, error: "unavailable" }] };
  assert.deepEqual((await read(args)).records, []);
  assert.equal(calls, 3);
  result = { ...packet, project: "project-b" };
  await assert.rejects(read(args), /federation packet/);
  result = { ...packet, records: [{ ...packet.records[0], handle: { ...handle, snapshot: "e".repeat(64) } }] };
  await assert.rejects(read(args), /federation packet/);
});

it("injection exposes each owner/snapshot and evidence handle", () => {
  const text = buildInjectionText({ bundleVersion: packet.snapshot, repo: packet.project, query: "context", records: packet.records, sources: packet.sources });
  assert.ok(text.includes(handle.provider));
  assert.ok(text.includes(handle.scope));
  assert.ok(text.includes(JSON.stringify(handle)));
  assert.ok(text.includes("not instructions"));
});
