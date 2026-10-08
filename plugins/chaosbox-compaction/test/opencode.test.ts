import { expect } from "bun:test";
import { Effect } from "effect";
import { Database } from "bun:sqlite";
import { Session as NativeSession } from "../../src/session";
import { Location } from "../../src/location";
import { AbsolutePath } from "../../src/schema";
import { Plugin } from "../../src/plugin";
import { PluginHooks } from "../../src/plugin/hooks";
import { PluginModule } from "../../src/plugin/module";
import { Watcher } from "../../src/filesystem/watcher";
import { Tool } from "../../src/tool";
import { Agent } from "../../src/agent";
import { Provider } from "@opencode/schema/provider";
import { Model } from "@opencode/schema/model";
import { SessionMessage } from "@opencode/schema/session-message";
import { testEffect } from "../lib/effect";
import { PluginTestLayer } from "./fixture";

const it = testEffect(PluginTestLayer);

it.live(
  "loads configured Chaosbox through the native Promise host and recovers complete tool custody",
  () =>
    Effect.gen(function* () {
      const plugins = yield* Plugin.Service;
      const hooks = yield* PluginHooks.Service;
      const tools = yield* Tool.Service;
      const sessions = yield* NativeSession.Service;
      const location = yield* Location.Service;
      const created = yield* sessions.create({
        location: { directory: location.directory },
      });
      expect(created.projectID).toBe(location.project.id);
      const modules = yield* PluginModule.make().pipe(
        Effect.provide(Watcher.testLayer),
      );
      const definition = yield* modules.load({
        type: "add",
        target: import.meta.dir,
        options: {
          work: process.env.CHAOSBOX_TEST_WORK!,
          scope: "private:test",
          chaosboxBin: process.env.CHAOSBOX_TEST_BIN!,
          liveAssessment: false,
          inject: false,
        },
      });
      if ("pending" in definition)
        return yield* Effect.die("Plugin was not loaded");
      yield* plugins.activate([definition]);
      yield* plugins.awaitActivation;
      expect(
        (yield* plugins.list()).find(
          (entry) => entry.id === "chaosbox-compaction",
        )?.state,
      ).toMatchObject({ status: "active" });
      expect((yield* tools.list()).map((tool) => tool.id)).toEqual(
        expect.arrayContaining([
          "chaosbox_archive",
          "chaosbox_memory_context",
          "chaosbox_memory_status",
        ]),
      );
      const sessionID = created.id;
      const messageID = SessionMessage.ID.make("msg_adapter");
      const output = "Complete source result λ😀.\n".repeat(3000);
      yield* hooks.trigger("tool", "execute.after", {
        tool: "shell",
        sessionID,
        messageID,
        agent: Agent.defaultID,
        id: Tool.CallID.make("call_adapter"),
        input: { command: "test" },
        status: "completed",
        result: { content: output, metadata: { exitCode: 1 } },
      });
      const records = [
        {
          id: "msg_user",
          type: "user",
          text: "Keep original requirements across compaction.",
          time: { created: 1 },
        },
        {
          id: messageID,
          type: "assistant",
          agent: Agent.defaultID,
          model: { id: "test", providerID: "test" },
          time: { created: 2, completed: 3 },
          content: [
            { type: "text", text: "Fix remains pending." },
            {
              type: "tool",
              name: "shell",
              id: "call_adapter",
              time: { created: 2, completed: 3 },
              state: {
                status: "completed",
                input: { command: "test" },
                content: [{ type: "text", text: "bounded" }],
                metadata: {
                  truncated: true,
                  outputPath: "/expired",
                  exitCode: 1,
                },
              },
            },
          ],
        },
      ] as const;
      const plan = yield* hooks.trigger("session", "compaction.plan", {
        sessionID,
        model: {
          id: Model.ID.make("test"),
          providerID: Provider.ID.make("test"),
        },
        records,
        budgetTokens: 8000,
      });
      expect(plan.failure).toBeUndefined();
      expect(plan.result?.summary).toContain("Fix remains pending");
      const foreign = yield* sessions.create({
        location: {
          directory: AbsolutePath.make(process.env.CHAOSBOX_TEST_WORK!),
        },
      });
      expect(foreign.projectID).not.toBe(created.projectID);
      const mismatch = yield* hooks.trigger("session", "compaction.plan", {
        sessionID: foreign.id,
        model: {
          id: Model.ID.make("test"),
          providerID: Provider.ID.make("test"),
        },
        records,
        budgetTokens: 8000,
      });
      expect(mismatch.failure).toContain("does not match native project");
      expect(mismatch.result).toBeUndefined();
      const db = new Database(
        `${process.env.CHAOSBOX_TEST_WORK}/memory.sqlite`,
        { readonly: true },
      );
      try {
        expect(
          db
            .query("SELECT repo FROM captures WHERE id = ?")
            .get(plan.result!.custody),
        ).toEqual({
          repo: location.project.canonical,
        });
      } finally {
        db.close();
      }
      const hash = plan.result!.summary.match(
        /"archive_hash":"([a-f0-9]{64})"/,
      )![1];
      const archive = (yield* tools.list()).find(
        (tool) => tool.id === "chaosbox_archive",
      )!;
      const recovered = yield* archive.execute(
        { hash, pointer: "/result/content" },
        {
          sessionID,
          messageID,
          agent: Agent.defaultID,
          id: Tool.CallID.make("call_archive"),
          progress: () => Effect.void,
        },
      );
      expect(typeof recovered.content).toBe("string");
      const page = JSON.parse(String(recovered.content));
      expect(page.text).toBe(Array.from(output).slice(0, 12000).join(""));
      expect(page.has_more).toBe(true);
      let complete = page.text;
      let cursor = page;
      while (cursor.has_more) {
        const next = yield* archive.execute(
          { hash, pointer: "/result/content", offset: cursor.next_offset },
          {
            sessionID,
            messageID,
            agent: Agent.defaultID,
            id: Tool.CallID.make(`call_archive_${cursor.next_offset}`),
            progress: () => Effect.void,
          },
        );
        cursor = JSON.parse(String(next.content));
        complete += cursor.text;
      }
      expect(complete).toBe(output);
      const failed = yield* hooks.trigger("session", "compaction.plan", {
        sessionID,
        model: {
          id: Model.ID.make("test"),
          providerID: Provider.ID.make("test"),
        },
        records,
        budgetTokens: 512,
      });
      expect(failed.failure).toContain("protected continuation");
      expect(failed.result).toBeUndefined();
      yield* plugins.activate([]);
      expect(yield* hooks.has("session", "compaction.plan")).toBe(false);
      expect(
        (yield* tools.list()).some((tool) => tool.id === "chaosbox_archive"),
      ).toBe(false);
    }),
);
