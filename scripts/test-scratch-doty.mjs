// Real-binary integration: plans retain custody configuration and refresh holds.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { createTracker, subprocess } from "../plugins/chaosbox-scratch/runtime.mjs";

const [chaosbox, doty] = process.argv.slice(2);
const liveJev = process.argv.includes("--live-jev");
assert.ok(chaosbox && doty, "usage: node scripts/test-scratch-doty.mjs /absolute/chaosbox /absolute/doty");
const base = await mkdtemp("/data/scratch/tmp/opencode/scratch-doty-test-");
const root = join(base, "scratch");
const work = join(base, "custody");
await mkdir(root);
const tracker = await createTracker({ root,work,scope:"private:test",host:"test",chaosboxBin:chaosbox });
const scratchArgs = ["scratch","--work",work,"--scope","private:test","--host","test","--root",root];
const dotyArgs = ["--chaosbox-work",work,"--chaosbox-scope","private:test","--chaosbox-host","test","--chaosbox-root",root,"--chaosbox-bin",chaosbox];
const env = { ...process.env, XDG_STATE_HOME:join(base,"doty-state"), CHAOSBOX_SCRATCH_ASSESS:String(liveJev) };
function run(args, success = true) {
  const result = spawnSync(doty,[...dotyArgs,...args],{ env,encoding:"utf8",timeout:70_000 });
  assert.equal(result.status===0,success, result.stderr || result.stdout);
  return success ? JSON.parse(result.stdout) : result.stderr;
}
try {
  const parent = join(root,"experiment");
  const child = join(parent,"patch");
  await mkdir(child,{ recursive:true });
  await tracker.reconcile();
  await tracker.note(child,"Integrate the patch into the repository");
  await subprocess(chaosbox,[...scratchArgs,"release",parent,"--reason","Parent contains disposable outputs"]);
  let preview = run(["rm","--root",root,"--json","--",parent]);
  if (liveJev) {
    const entry = preview.scratch_intelligence.items[0].entries.find(e => e.path === child);
    assert.equal(entry.assessment_state,"current");
    assert.equal(entry.assessment.fresh,true);
    const receipt = await subprocess(chaosbox,[...scratchArgs,"receipt",entry.assessment.id]);
    assert.ok(entry.assessment.obligations.length, JSON.stringify(receipt.responses));
    assert.equal(entry.assessment.release_recommended,false);
    assert.equal(receipt.id,entry.assessment.id);
    assert.ok(receipt.input && receipt.responses);
  }
  assert.equal(preview.scratch_intelligence.items[0].blocked,true);
  assert.match(run(["rm","--apply","--plan",preview.id,"--json"],false),/needs-finalization/);
  await subprocess(chaosbox,[...scratchArgs,"release",child,"--reason","Patch integrated","--receipt","commit:fixture"]);
  preview = run(["rm","--root",root,"--json","--",parent]);
  assert.equal(preview.scratch_intelligence.items[0].blocked,false);
  await tracker.note(child,"Follow-up regression test is still pending");
  assert.match(run(["rm","--apply","--plan",preview.id,"--json"],false),/needs-finalization/);
  await subprocess(chaosbox,[...scratchArgs,"release",child,"--reason","Regression test integrated"]);
  const applied = run(["rm","--apply","--plan",preview.id,"--json"]);
  assert.ok(applied.targets[0].quarantined_as);
  const purged = run(["purge","--apply",preview.id,"--json"]);
  assert.equal(purged.targets[0].purged,true);
  console.log("PASS: nested finalization holds, apply-time refresh, quarantine identity and purge");
} finally {
  await tracker.close();
  await rm(base,{ recursive:true,force:true });
}
