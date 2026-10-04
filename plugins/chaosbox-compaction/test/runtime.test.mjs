import { test } from "node:test";
import assert from "node:assert/strict";
import { coordinator, subprocess } from "../runtime.mjs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const options = {
  work: "/private/memory",
  scope: "private:can",
  repo: "chaosbox",
  inject: false,
};
test("requires complete custody and never silently delegates a failed reduction", async () => {
  const memory = coordinator(options, async () => {
    throw new Error("disk full");
  });
  const event = {
    sessionID: "ses_test",
    records: [{ id: "msg_1", type: "user", text: "Exact requirement" }],
    budgetTokens: 10000,
  };
  await memory.compact(event);
  assert.match(event.failure, /disk full/);
  assert.equal(event.result, undefined);
});
test("installs the returned tail and receipt only after native capture", async () => {
  const calls = [];
  const memory = coordinator(options, async (binary, args, input) => {
    calls.push({ args, input });
    return {
      summary: "Exact requirement",
      recent: "",
      custody: "a".repeat(64),
      id: "b".repeat(64),
    };
  });
  const native = [{ id: "msg_1", type: "user", text: "Exact requirement" }];
  const event = { sessionID: "ses_test", records: native, budgetTokens: 10000 };
  await memory.compact(event);
  assert.deepEqual(calls[0].input.records, native);
  assert.deepEqual(calls[0].args.slice(0, 2), ["memory", "compact"]);
  assert.equal(event.result.custody, "a".repeat(64));
  assert.equal(event.result.recent, "");
});
test("captures full tool settlement before bounding and excludes retrieval feedback", async () => {
  const calls = [];
  const memory = coordinator(options, async (_, args, input) => {
    calls.push({ args, input });
    return {};
  });
  const event = {
    sessionID: "ses_test",
    messageID: "msg_1",
    id: "call_1",
    tool: "shell",
    input: { command: "test" },
    status: "completed",
    result: {
      content: [{ type: "text", text: "λ".repeat(100000) }],
      metadata: { exitCode: 1 },
    },
  };
  await memory.tool(event);
  assert.deepEqual(calls[0].input.records[0].result, event.result);
  await memory.tool({ ...event, tool: "chaosbox_archive" });
  assert.equal(calls.length, 1);
});
test("tools cannot broaden scope, repository or select archive paths", async () => {
  const calls = [];
  const memory = coordinator(options, async (_, args) => {
    calls.push(args);
    return {};
  });
  await assert.rejects(memory.archive({ hash: "../../private/file" }));
  await memory.retrieve({
    query: "active constraint",
    scope: "public",
    repo: "other",
  });
  assert.deepEqual(calls[0], [
    "memory",
    "context",
    "--scope",
    "private:can",
    "--repo",
    "chaosbox",
    "active constraint",
  ]);
});

test("settled boundary capture preserves final replies and job notifications", async () => {
  const calls = [];
  const memory = coordinator(options, async (_, args, input) => {
    calls.push({ args, input });
    return {};
  });
  const user = {
    id: "msg_user",
    type: "user",
    text: "Keep current requirements.",
  };
  await memory.settled("ses_test", [
    user,
    {
      id: "msg_streaming",
      type: "assistant",
      time: { created: 1 },
      content: [],
    },
  ]);
  assert.deepEqual(calls[0].input.records, [user]);
  async function* events() {
    yield { type: "session.step.ended", data: { sessionID: "ses_other" } };
    yield { type: "session.step.ended", data: { sessionID: "ses_test" } };
    yield { type: "session.synthetic", data: { sessionID: "ses_test" } };
  }
  const reply = {
    id: "msg_final",
    type: "assistant",
    time: { created: 1, completed: 2 },
    content: [{ type: "text", text: "Implementation still blocked." }],
  };
  const notice = {
    id: "msg_job",
    type: "synthetic",
    text: "Build completed with exit code 1.",
  };
  await memory.observe(events(), async () => [user, reply, notice]);
  assert.equal(calls.length, 3);
  assert.deepEqual(calls[1].input.records, [user, reply, notice]);
});

test(
  "real Rust subprocess preserves Unicode, complete outputs and restart identity",
  { skip: !process.env.CHAOSBOX_TEST_BIN },
  async () => {
    const work = await mkdtemp(join(tmpdir(), "chaosbox-adapter-"));
    try {
      const config = {
        ...options,
        work,
        chaosboxBin: process.env.CHAOSBOX_TEST_BIN,
      };
      const memory = coordinator(config, subprocess);
      const result = "λ😀".repeat(30000);
      await memory.tool({
        sessionID: "ses_real",
        messageID: "msg_tool",
        id: "call_real",
        tool: "shell",
        input: { command: "test" },
        status: "completed",
        result: { content: result, metadata: { exitCode: 1 } },
      });
      const records = [
        {
          id: "msg_user",
          type: "user",
          text: "Keep exact requirements including λ😀.",
          time: { created: 1 },
        },
        {
          id: "msg_tool",
          type: "assistant",
          time: { created: 2, completed: 3 },
          content: [
            { type: "text", text: "Repair remains pending." },
            {
              type: "tool",
              id: "call_real",
              name: "shell",
              state: {
                status: "completed",
                input: { command: "test" },
                content: [{ type: "text", text: "bounded preview" }],
                metadata: {
                  truncated: true,
                  outputPath: "/ephemeral/expired",
                  exitCode: 1,
                },
              },
            },
          ],
        },
      ];
      const event = { sessionID: "ses_real", records, budgetTokens: 8000 };
      await memory.compact(event);
      assert.equal(event.failure, undefined);
      assert.match(event.result.summary, /λ😀/);
      assert.match(event.result.summary, /pending/);
      const hash = event.result.summary.match(
        /"archive_hash":"([a-f0-9]{64})"/,
      )[1];
      const page = await memory.archive({ hash, pointer: "/result/content" });
      assert.equal(page.text, Array.from(result).slice(0, 12000).join(""));
      assert.equal(page.has_more, true);
      const next = await memory.archive({
        hash,
        pointer: "/result/content",
        offset: page.next_offset,
      });
      assert.equal(next.text, Array.from(result).slice(12000, 24000).join(""));
      const restarted = coordinator(config, subprocess);
      const repeated = { sessionID: "ses_real", records, budgetTokens: 8000 };
      await restarted.compact(repeated);
      assert.deepEqual(repeated.result, event.result);
      await restarted.compact({ ...repeated, budgetTokens: 512 });
      assert.equal((await restarted.status()).degraded, true);
    } finally {
      await rm(work, { recursive: true, force: true });
    }
  },
);
