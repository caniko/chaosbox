import { expect } from "bun:test";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { Effect } from "effect";
import { Plugin } from "../../../src/plugin";
import { PluginHooks } from "../../../src/plugin/hooks";
import { PluginModule } from "../../../src/plugin/module";
import { Watcher } from "../../../src/filesystem/watcher";
import { Tool } from "../../../src/tool";
import { Agent } from "../../../src/agent";
import { Session as NativeSession } from "../../../src/session";
import { Location } from "../../../src/location";
import { Session } from "@opencode/schema/session";
import { SessionMessage } from "@opencode/schema/session-message";
import { testEffect } from "../../lib/effect";
import { PluginTestLayer } from "../../plugin/fixture";

const it = testEffect(PluginTestLayer);

it.live("native Promise host loads scratch tools and retains finalization assertions", () => Effect.gen(function* () {
  const plugins = yield* Plugin.Service;
  const hooks = yield* PluginHooks.Service;
  const tools = yield* Tool.Service;
  const modules = yield* PluginModule.make().pipe(Effect.provide(Watcher.testLayer));
  const root = process.env.CHAOSBOX_TEST_ROOT!;
  const definition = yield* modules.load({
    type: "add", target: join(import.meta.dir, "index.ts"),
    options: { root, work: process.env.CHAOSBOX_TEST_WORK!, scope: "private:test", host: "test", chaosboxBin: process.env.CHAOSBOX_TEST_BIN! },
  });
  if ("pending" in definition) return yield* Effect.die("Scratch adapter did not load");
  yield* plugins.activate([definition]);
  yield* plugins.awaitActivation;
  expect((yield* plugins.list()).find(p => p.id === "chaosbox-scratch")?.state).toMatchObject({ status: "active" });
  const folder = join(root, "native-test");
  yield* Effect.promise(() => mkdir(folder));
  const note = (yield* tools.list()).find(t => t.id === "chaosbox_scratch_note")!;
  const context = { sessionID: Session.ID.make("ses_scratch"),messageID: SessionMessage.ID.make("msg_scratch"),agent: Agent.defaultID,id: Tool.CallID.make("call_scratch"),progress: () => Effect.void };
  yield* note.execute({ path:folder,reason:"Integrate the recovered patch",disposition:"needs-finalization" },context);
  const explain = (yield* tools.list()).find(t => t.id === "chaosbox_scratch_explain")!;
  const result = yield* explain.execute({ path:folder },context);
  const packet = JSON.parse(String(result.content));
  expect(packet.items[0].blocked).toBe(true);
  expect(packet.items[0].entries[0].annotations[0].reason).toBe("Integrate the recovered patch");
  const sessions = yield* NativeSession.Service;
  const location = yield* Location.Service;
  const session = yield* sessions.create({location:Location.Ref.make({directory:location.directory})});
  const workspace = (yield* tools.list()).find(t => t.id === "chaosbox_scratch_workspace")!;
  const allocation = JSON.parse(String((yield* workspace.execute({purpose:"Retain this reproduction for integration"},{...context,sessionID:session.id,id:Tool.CallID.make("call_workspace")})).content));
  expect(allocation.path.startsWith(root)).toBe(true);
  const created = JSON.parse(String((yield* explain.execute({path:allocation.path},context)).content));
  expect(created.items[0].entries[0].invocations[0].session).toBe(session.id);
  expect(created.items[0].entries[0].annotations[0].reason).toBe("Retain this reproduction for integration");
  const link = (yield* tools.list()).find(t => t.id === "chaosbox_scratch_link")!;
  yield* link.execute({path:allocation.path,category:"patch",reference:"patch:fixture",description:"Proposed fix; integration still pending"},{...context,sessionID:session.id});
  const linked = JSON.parse(String((yield* explain.execute({path:allocation.path},context)).content));
  expect(linked.items[0].entries[0].links[0]).toMatchObject({session:session.id,message:context.messageID,verified:false});
  yield* plugins.activate([]);
  expect(yield* hooks.has("tool","execute.before")).toBe(false);
  expect((yield* tools.list()).some(t => t.id === "chaosbox_scratch_note")).toBe(false);
}));
