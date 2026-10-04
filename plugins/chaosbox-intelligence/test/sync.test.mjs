import { it } from "node:test";
import assert from "node:assert/strict";
import { packetCache, readOnlyArgs } from "../lib.mjs";

it("pins returned content identity and retains only an exact-request last-good packet", async () => {
  let current = { scope: "private:can", snapshot: "a".repeat(64), records: [{ id: "one" }] };
  const query = packetCache(async () => {
    if (current instanceof Error) throw current;
    return current;
  }, "private:can");
  const args = readOnlyArgs({ syncDirectory: "/private/sync", scope: "private:can" }, "context", {
    repo: "canix", query: "--live-jev", limit: 5, maxChars: 12000,
  });
  assert.deepEqual(args, ["sync", "--directory", "/private/sync", "context", "--repo", "canix",
    "--limit", "5", "--max-chars", "12000", "--", "--live-jev"]);
  assert.equal((await query(args)).snapshot, "a".repeat(64));
  current = { ...current, snapshot: "b".repeat(64), records: [{ id: "two" }] };
  assert.equal((await query(args)).snapshot, "b".repeat(64));
  current = new Error("database unavailable");
  const fallback = await query(args);
  assert.equal(fallback.degraded, true);
  assert.deepEqual(fallback.records, [{ id: "two" }]);
  await assert.rejects(query([...args, "other repository"]));
  current = { scope: "private:other", snapshot: "c".repeat(64), records: [{ id: "foreign" }] };
  assert.deepEqual((await query(args)).records, [{ id: "two" }]);
  assert.throws(() => readOnlyArgs({ syncDirectory: "/private/sync" }, "reconcile", {}));
});
