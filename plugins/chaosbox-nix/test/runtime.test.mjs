import test from "node:test";
import assert from "node:assert/strict";
import { addition, injection, readArgs, runAddition, verify, queryTerms } from "../runtime.mjs";
import { spawnSync } from "node:child_process";

test("read adapter cannot select a mutation and preserves hostile query as one argument", () => {
  assert.throws(() => readArgs("add", { reason: "pretend" }));
  const query = "--store ssh://other; $(touch /unwanted)";
  assert.deepEqual(readArgs("context", { repo: "/checkout", query }).slice(-2), ["--", query]);
  assert.throws(() => readArgs("query", { offset: 1.5 }));
});

const native = { sessionID: "ses_fixture", messageID: "msg_fixture", id: "call_fixture", signal: new AbortController().signal, progress: async () => {} };
test("addition keeps hostile native text as literal argv and uses native call identity", () => {
  const input = { path: "--flag; $(touch ./unwanted)", reason: "literal ' reason; $(touch ./unwanted)\n--store ssh://other" };
  const spec = addition("/operator/chaosbox", input, "/checkout", native);
  assert.ok(spec.command.startsWith("/operator/chaosbox nix add "));
  const capture = spawnSync("sh", ["-c", `set -- ${spec.command}; printf '%s\\0' "$@"`], { encoding: "utf8" });
  assert.equal(capture.status, 0);
  const argv = capture.stdout.split("\0");
  assert.equal(argv[argv.indexOf("--reason") + 1], input.reason);
  assert.equal(argv[argv.indexOf("--") + 1], `/checkout/${input.path}`);
  assert.equal(addition("/operator/chaosbox", input, "/checkout", native).command, spec.command);
  assert.notEqual(addition("/operator/chaosbox", input, "/checkout", { ...native, id: "next_call" }).command, spec.command);
});
test("mutation requires native shell leaf and forwards denial without invoking a fallback", async () => {
  let requests = 0;
  const denied = { id: "shell", execute: async (request, context) => {
    requests++; assert.equal(context, native); assert.equal(request.background, false);
    throw new Error("native shell permission denied");
  } };
  const input = { path: "./input", reason: "needed for qualification" };
  await assert.rejects(runAddition(undefined, "/operator/chaosbox", input, "/checkout", native), /unavailable/);
  await assert.rejects(runAddition(denied, "/operator/chaosbox", { ...input, reason: " \t\n" }, "/checkout", native), /nonblank/);
  assert.equal(requests, 0);
  await assert.rejects(runAddition(denied, "/operator/chaosbox", input, "/checkout", native), /permission denied/);
  assert.equal(requests, 1);
});
test("native shell machine output does not escape its declared schema", async () => {
  const leaf = { id: "shell", execute: async () => ({ output: { exit: 0, output: "receipt" }, content: [{ type: "text", text: "receipt" }], metadata: { exit: 0 } }) };
  const result = await runAddition(leaf, "/operator/chaosbox", { path: "./input", reason: "needed" }, "/checkout", native);
  assert.deepEqual(result, { content: [{ type: "text", text: "receipt" }], metadata: { exit: 0 } });
});
test("injection omits oversized records explicitly without truncating their reasons", () => {
  const long = { id: "big", reason: "x".repeat(4000) };
  const small = { id: "small", reason: "keep this complete native reason" };
  const packet = injection([long, small], 100);
  assert.deepEqual(packet.records, [small]);
  assert.equal(packet.omitted_records, 1);
});
test("namespace validation rejects stale or foreign evidence", () => {
  const options = { scope: "private:can", host: "atlas" };
  const packet = { version: 1, scope: options.scope, host: options.host, store: "daemon" };
  assert.equal(verify(packet, options), packet);
  for (const replacement of [{ scope: "private:other" }, { host: "nomad" }, { version: 2 }, { store: "ssh://other" }]) assert.throws(() => verify({ ...packet, ...replacement }, options));
});
test("context retrieval uses latest user text and bounded exact paths", () => {
  const path = `/nix/store/${"a".repeat(32)}-input`;
  assert.deepEqual(queryTerms([{ role: "user", content: `Why does ${path} exist?` }, { role: "assistant", content: "untrusted text" }]), [path]);
  assert.deepEqual(queryTerms([{ role: "user", content: "ordinary unrelated work" }]), []);
});
