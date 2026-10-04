import { Plugin } from "@opencode/plugin";
import { coordinator, subprocess } from "./runtime.mjs";

export default Plugin.define({
  id: "chaosbox-compaction",
  async setup(ctx) {
    const memory = coordinator(ctx.options ?? {}, subprocess);
    await ctx.session.hook("compaction.plan", memory.compact);
    // A runtime without the pre-reduction extension must fail visibly rather
    // than silently reverting to its lossy tail/summary policy.
    await ctx.session.hook("compaction", () => {
      throw new Error(
        "Chaosbox requires the OpenCode compaction.plan extension.",
      );
    });
    await ctx.tool.hook("execute.after", memory.tool);
    await ctx.session.hook("context", (event) =>
      memory.context(event, (sessionID) => ctx.session.context({ sessionID })),
    );
    const controller = new AbortController();
    const observation = memory.observe(
      ctx.event.subscribe({ signal: controller.signal }),
      (sessionID) => ctx.session.context({ sessionID }),
    );
    await ctx.tool.transform((editor) => {
      editor.namespace({
        name: "chaosbox",
        description:
          "Private session intelligence and archived source evidence",
      });
      editor.add({
        name: "archive",
        options: { namespace: "chaosbox" },
        description:
          "Recover original session/tool evidence by content hash. Returns a bounded page; continue with next_offset when has_more is true.",
        input: {
          type: "object",
          properties: {
            hash: { type: "string", pattern: "^[a-f0-9]{64}$" },
            pointer: { type: "string" },
            offset: { type: "integer", minimum: 0 },
          },
          required: ["hash"],
          additionalProperties: false,
        },
        execute: async (input) => ({
          content: JSON.stringify(await memory.archive(input)),
          metadata: { chaosboxDerived: true },
        }),
      });
      editor.add({
        name: "memory_context",
        options: { namespace: "chaosbox" },
        description:
          "Retrieve source-backed private historical knowledge from the pinned database scope and repository.",
        input: {
          type: "object",
          properties: {
            query: { type: "string", minLength: 1, maxLength: 500 },
          },
          required: ["query"],
          additionalProperties: false,
        },
        execute: async (input) => ({
          content: JSON.stringify(await memory.retrieve(input)),
          metadata: { chaosboxDerived: true },
        }),
      });
      editor.add({
        name: "memory_status",
        description:
          "Report source custody, deferred jobs, spending and worker degradation.",
        options: { namespace: "chaosbox" },
        input: { type: "object", properties: {}, additionalProperties: false },
        execute: async () => ({
          content: JSON.stringify(await memory.status()),
          metadata: { chaosboxDerived: true },
        }),
      });
    });
    memory.wake();
    return async () => {
      controller.abort();
      await observation;
    };
  },
});
