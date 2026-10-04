import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setImmediate } from "node:timers/promises";
import { createTracker } from "../runtime.mjs";

async function fixture(run, options = {}) {
  const base = await mkdtemp(join(tmpdir(), "scratch-scheduling-"));
  const root = join(base, "scratch");
  await mkdir(root);
  try {
    const tracker = await createTracker({ root, work: join(base, "custody"), scope: "private:test", host: "test", liveAssessment: true, ...options }, run);
    let closed = false;
    return { tracker, root, async close() {
      if (closed) return;
      closed = true;
      await tracker.close();
      await rm(base, { recursive: true, force: true });
    } };
  } catch (error) {
    await rm(base, { recursive: true, force: true });
    throw error;
  }
}

const allocation = { id: "allocate", sessionID: "ses", messageID: "msg", tool: "chaosbox_scratch_workspace" };

test("assessment wakes coalesce; evidence arriving during inference queues one bounded follow-up", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const gate = Promise.withResolvers();
  const events = [];
  const calls = [];
  let active = 0;
  const f = await fixture(async (_binary, args, input, timeout) => {
    if (input?.events) events.push(...input.events);
    if (!args.includes("assess")) return {};
    calls.push({ args, timeout });
    assert.equal(++active, 1, "workers must not overlap");
    if (calls.length === 1) await gate.promise;
    active--;
    return {};
  }, { maxRequests: 7, maxInputTokens: 12345 });
  try {
    await f.tracker.workspace("Save patch for review", allocation, "repo", f.root, []);
    await f.tracker.settled("ses", [{ id: "first", type: "user", text: "Review the patch" }]);
    t.mock.timers.tick(999);
    assert.equal(calls.length, 0);
    t.mock.timers.tick(1);
    assert.equal(calls.length, 1, "startup, allocation and context wakes coalesce");
    assert.deepEqual(calls[0].args.slice(-6), ["assess", "--privacy-reviewed", "--max-requests", "7", "--max-input-tokens", "12345"]);
    assert.equal(calls[0].timeout, 120_000);
    await f.tracker.settled("ses", [{ id: "later", type: "user", text: "Preserve the regression evidence" }]);
    await f.tracker.settled("ses", [{ id: "last", type: "user", text: "Integration still needs review" }]);
    assert.ok(events.some(e => e.kind === "context" && JSON.stringify(e).includes("Integration")));
    assert.equal(f.tracker.status().assessment.pending, true);
    await f.tracker.reconcile();
    t.mock.timers.tick(5000);
    assert.equal(calls.length, 1, "capture stays responsive without starting a second worker");
    gate.resolve();
    await setImmediate();
    t.mock.timers.tick(1000);
    await setImmediate();
    assert.equal(calls.length, 2, "all in-flight evidence wakes share one follow-up");
    assert.equal(f.tracker.status().assessment.pending, false);
    assert.equal(f.tracker.status().assessment.running, false);
  } finally { gate.resolve(); await f.close(); }
});

test("failed inference is visible without degrading custody or retrying in a loop", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const events = [];
  let calls = 0;
  const f = await fixture(async (_binary, args, input) => {
    if (input?.events) events.push(...input.events);
    if (args.includes("assess") && ++calls === 1) throw new Error("Jev offline");
    return {};
  });
  try {
    t.mock.timers.tick(1000);
    await setImmediate();
    assert.match(f.tracker.status().assessment.lastError, /Jev offline/);
    assert.equal(f.tracker.status().assessment.running, false);
    assert.equal(f.tracker.status().degraded, false);
    t.mock.timers.tick(10_000);
    await setImmediate();
    assert.equal(calls, 1, "a failed worker must not create a busy retry loop");
    await f.tracker.workspace("Keep unfinished investigation", allocation, "repo", f.root, []);
    assert.ok(events.some(e => e.kind === "annotation" && e.reason.includes("unfinished")));
    t.mock.timers.tick(1000);
    await setImmediate();
    assert.equal(calls, 2, "new evidence can wake the durable worker");
    assert.equal(f.tracker.status().assessment.lastError, "");
  } finally { await f.close(); }
});

test("remaining work reschedules a drain and unloading cancels a queued wake", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let calls = 0;
  const f = await fixture(async (_binary, args) => {
    if (args.includes("assess")) { calls++; return { remaining: true }; }
    return {};
  });
  try {
    t.mock.timers.tick(1000);
    await setImmediate();
    assert.equal(calls, 1);
    assert.equal(f.tracker.status().assessment.pending, true);
    t.mock.timers.tick(1000);
    await setImmediate();
    assert.equal(calls, 2);
    await f.close();
    t.mock.timers.tick(10_000);
    await setImmediate();
    assert.equal(calls, 2, "unloading cancels the third scheduled drain");
  } finally { await f.close(); }
});

test("disabled assessment still captures allocation purpose and never dispatches Jev", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const events = [];
  let calls = 0;
  const f = await fixture(async (_binary, args, input) => {
    if (input?.events) events.push(...input.events);
    if (args.includes("assess")) calls++;
    return {};
  }, { liveAssessment: false });
  try {
    await f.tracker.workspace("Preserve experiment results", allocation, "repo", f.root, []);
    await f.tracker.settled("ses", [{ id: "context", type: "user", text: "Finalization required" }]);
    t.mock.timers.tick(10_000);
    await setImmediate();
    assert.equal(calls, 0);
    assert.equal(f.tracker.status().assessment.enabled, false);
    assert.ok(events.some(e => e.kind === "annotation" && e.reason.includes("experiment")));
    assert.ok(events.some(e => e.kind === "context"));
  } finally { await f.close(); }
});
