import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createTracker, purposeSources } from "../runtime.mjs";

test("native purpose fields retain citations and exclude derived text", () => {
  const sources = purposeSources([
    { id: "msg_u", type: "user", text: "Preserve the patch" },
    { id: "msg_summary", type: "compaction", summary: "Everything done" },
    { id: "msg_a", type: "assistant", content: [
      { type: "reasoning", text: "hidden" },
      { type: "text", text: "Reproduce the failing test" },
    ] },
    { id: "msg_tool", type: "assistant", content: [{type:"tool",name:"shell"}] },
  ]);
  assert.equal(sources.length, 2);
  assert.equal(sources[1].pointer, "/content/1/text");
  assert.equal(sources[1].record.id, "msg_a");
  assert.ok(!JSON.stringify(sources).includes("Everything done"));
});

test("observes indirect creation; background ownership persists until shell exit", async () => {
  const base = await mkdtemp(join(tmpdir(), "chaosbox-scratch-test-"));
  const root = join(base, "scratch");
  const work = join(base, "custody");
  await mkdir(root);
  const batches = [];
  const tracker = await createTracker({ root, work, scope: "private:test", host: "test" }, async (_bin, _args, input) => {
    if (input?.events) batches.push(...input.events);
    return {};
  });
  try {
    await tracker.before({ id: "call-a", messageID: "msg_a", sessionID: "ses_a", tool: "shell", input: { command: "python script.py", background: true } },
      "repo", root, [{ id: "msg_u", type: "user", text: "Keep results for integration" }]);
    await tracker.event({ type: "shell.created", data: { info: { id: "sh_a", command: "python script.py", cwd: root, metadata: { sessionID: "ses_a" } } } });
    await tracker.after({ id: "call-a", messageID: "msg_a", sessionID: "ses_a", tool: "shell", status: "completed", result: { metadata: { status: "running", shellID: "sh_a" } } });
    const folder = join(root, "indirect-output");
    await mkdir(folder);
    await tracker.reconcile();
    const observation = batches.find(e => e.kind === "observe" && e.path === folder && e.present);
    assert.ok(observation);
    assert.ok(observation.owners.includes("shell:sh_a"));
    assert.ok(!batches.some(e => e.kind === "end" && e.invocation === "shell:sh_a"));
    await tracker.event({ type: "shell.exited", data: { id: "sh_a", status: "exited", exit: 1 } });
    assert.ok(batches.some(e => e.kind === "end" && e.invocation === "shell:sh_a" && e.exit === 1));
    assert.ok(!batches.some(e => e.kind === "annotation" && e.disposition === "released"));
  } finally {
    await tracker.close();
    await rm(base, { recursive: true, force: true });
  }
});

test("concurrent commands remain possible owners and a failed writer is visible", async () => {
  const base = await mkdtemp(join(tmpdir(), "chaosbox-scratch-test-"));
  const root = join(base, "scratch");
  await mkdir(root);
  const batches = [];
  let unavailable = false;
  const tracker = await createTracker({ root, work: join(base, "custody"), scope: "private:test", host: "test" }, async (_bin, _args, input) => {
    if (unavailable) throw new Error("writer unavailable");
    if (input?.events) batches.push(...input.events);
    return {};
  });
  try {
    for (const id of ["a", "b"]) await tracker.before({ id, messageID: "msg", sessionID: `ses_${id}`, tool: "shell", input: { command: "script" } }, "repo", root, []);
    await mkdir(join(root, "shared-output"));
    await tracker.reconcile();
    const observed = batches.find(e => e.kind === "observe" && e.path.endsWith("shared-output"));
    assert.equal(observed.owners.length, 2);
    unavailable = true;
    await assert.rejects(tracker.reconcile(), /writer unavailable/);
    assert.equal(tracker.status().degraded, true);
    unavailable = false;
    await tracker.reconcile();
  } finally {
    unavailable = false;
    await tracker.close();
    await rm(base, { recursive: true, force: true });
  }
});

test("refuses custody within scratch and symlink roots", async () => {
  await assert.rejects(createTracker({ root: "/tmp/scratch", work: "/tmp/scratch/ledger", scope: "private:test" }, async () => ({})), /separate/);
  const base = await mkdtemp(join(tmpdir(), "chaosbox-scratch-test-"));
  try {
    await mkdir(join(base,"real"));
    await symlink(join(base,"real"),join(base,"link"));
    await assert.rejects(createTracker({ root:join(base,"link"),work:join(base,"custody"),scope:"private:test" },async () => ({})),/symlink/);
  } finally { await rm(base,{ recursive:true,force:true }); }
});

test("nested generated-file activity is aggregated and startup invalidates old releases", async () => {
  const base = await mkdtemp(join(tmpdir(),"chaosbox-scratch-test-"));
  const root = join(base,"scratch");
  const folder = join(root,"work");
  const nested = join(folder,"target","debug");
  await mkdir(nested,{ recursive:true });
  const events = [];
  const tracker = await createTracker({ root,work:join(base,"custody"),scope:"private:test",host:"test" },async (_bin,_args,input) => {
    if (input?.events) events.push(...input.events);
    return {};
  });
  try {
    assert.ok(events.some(e => e.kind==="observe" && e.path===folder && e.activity));
    assert.ok(events.some(e => e.kind==="coverage" && !e.healthy && /startup/.test(e.detail)));
    events.length = 0;
    await writeFile(join(nested,"artifact"),"generated");
    for (let attempt=0;attempt<20 && !events.some(e => e.kind==="observe" && e.path===folder && e.activity);attempt++) {
      await new Promise(resolve => setTimeout(resolve,10));
      await tracker.reconcile();
    }
    assert.ok(events.some(e => e.kind==="observe" && e.path===folder && e.activity));
    assert.ok(!events.some(e => e.kind==="observe" && e.path.includes("target")));
  } finally { await tracker.close(); await rm(base,{ recursive:true,force:true }); }
});

test("native user prompt parts retain their original JSON pointers", () => {
  const sources = purposeSources([{id:"msg_user",type:"user",prompt:{parts:[{type:"text",text:"Finalize this experiment"}]}}]);
  assert.equal(sources[0].pointer,"/prompt/parts/0/text");
  assert.equal(sources[0].record.prompt.parts[0].text,"Finalize this experiment");
});

test("Jev assessment is asynchronous, settled context is captured, and workspace purpose is bound", async () => {
  const base = await mkdtemp(join(tmpdir(),"chaosbox-scratch-test-"));
  const root = join(base,"scratch");
  await mkdir(root);
  const events = [];
  let unblock;
  let started;
  const dispatched = new Promise(resolve => { started = resolve; });
  const gate = new Promise(resolve => { unblock = resolve; });
  const tracker = await createTracker({root,work:join(base,"custody"),scope:"private:test",host:"test",liveAssessment:true},async (_binary,args,input) => {
    if (input?.events) events.push(...input.events);
    if (args.includes("assess")) { started(); await gate; }
    return {};
  });
  try {
    const event = {id:"allocate",messageID:"msg",sessionID:"ses",tool:"chaosbox_scratch_workspace"};
    const result = await tracker.workspace("Preserve the patch for integration",event,"repo",root,[{id:"user",type:"user",text:"Integrate the patch"}]);
    assert.ok(result.path.startsWith(root));
    assert.ok(events.some(e => e.kind === "begin" && e.session === "ses"));
    assert.ok(events.some(e => e.kind === "annotation" && e.reason.includes("integration")));
    await tracker.settled("ses",[{id:"done",type:"assistant",content:[{type:"text",text:"The regression test still needs to run"}]}]);
    await dispatched;
    assert.ok(events.some(e => e.kind === "context" && JSON.stringify(e).includes("regression")));
    assert.equal(tracker.status().assessment.running,true);
    await tracker.reconcile(); // a running Jev request must not block capture
  } finally { unblock(); await tracker.close(); await rm(base,{recursive:true,force:true}); }
});
