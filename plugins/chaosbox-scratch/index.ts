import { Plugin } from "@opencode/plugin";
import { acquireTracker } from "./runtime.mjs";

export default Plugin.define({
  id: "chaosbox-scratch",
  async setup(ctx) {
    const { tracker, release } = await acquireTracker(ctx.options ?? {});
    const controller = new AbortController();
    const report = async (error: unknown) => {
      console.error(`Chaosbox scratch capture degraded: ${String(error)}`);
      try { await tracker.failure(error); } catch { /* status retains the failure; heartbeat expires */ }
    };
    await ctx.tool.hook("execute.before", async event => {
      if (event.tool.startsWith("chaosbox_")) return;
      try {
        const session = await ctx.session.get({ sessionID: event.sessionID });
        const records = await ctx.session.context({ sessionID: event.sessionID });
        // Resolve each session's placement; plugin instance location is not session identity.
        const directory = session.location.directory;
        await tracker.before(event, directory, directory, records);
      } catch (error) { await report(error); }
    });
    await ctx.tool.hook("execute.after", async event => {
      try { await tracker.after(event); } catch (error) { await report(error); }
    });
    const observation = (async () => {
      try {
        for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
          if (event.type === "session.step.ended" || event.type === "session.step.failed" || event.type === "session.execution.succeeded" || event.type === "session.execution.failed" || event.type === "session.execution.interrupted") {
            if (!tracker.hasSession(event.data.sessionID)) continue;
            try { await tracker.settled(event.data.sessionID, await ctx.session.context({sessionID:event.data.sessionID})); }
            catch (error) { await report(error); }
          }
          if (!event.type.startsWith("shell.")) continue;
          try { await tracker.event(event); } catch (error) { await report(error); }
        }
      } catch (error) { if (!controller.signal.aborted) await report(error); }
    })();
    await ctx.tool.transform(editor => {
      editor.namespace({ name: "chaosbox", description: "Scratch provenance and unfinished-work intelligence" });
      editor.add({
        name:"scratch_workspace", options:{namespace:"chaosbox"},
        description:"Create a scratch workspace with a required purpose, bound to this session and message. Returns its path; ordinary command-created workspaces are also captured automatically.",
        input:{type:"object",properties:{purpose:{type:"string",minLength:1,maxLength:4000}},required:["purpose"],additionalProperties:false},
        execute: async (input, context) => {
          const session = await ctx.session.get({sessionID:context.sessionID});
          const records = await ctx.session.context({sessionID:context.sessionID});
          const result = await tracker.workspace((input as {purpose:string}).purpose,
            {sessionID:context.sessionID,messageID:context.messageID,id:context.id,tool:"chaosbox_scratch_workspace"},
            session.location.directory,session.location.directory,records);
          return {content:JSON.stringify(result),metadata:{chaosboxDerived:true}};
        },
      });
      editor.add({
        name:"scratch_link",options:{namespace:"chaosbox"},
        description:"Link a patch, test, commit, artifact or preservation reference to scratch work. References are assertions; Jev assesses them with linked source/command evidence.",
        input:{type:"object",properties:{path:{type:"string"},category:{type:"string",enum:["patch","test","commit","artifact","preservation"]},reference:{type:"string",minLength:1,maxLength:2000},description:{type:"string",minLength:1,maxLength:4000}},required:["path","category","reference","description"],additionalProperties:false},
        execute: async (input, context) => {
          const args = input as {path:string;category:string;reference:string;description:string};
          return {content:JSON.stringify(await tracker.link(args.path,args.category,args.reference,args.description,context)),metadata:{chaosboxDerived:true}};
        },
      });
      editor.add({
        name: "scratch_note",
        options: { namespace: "chaosbox" },
        description: "Record why a scratch work folder exists and what still needs finalization. Generated contents are aggregated under it. This is an assertion, not a release or deletion authorization.",
        input: {
          type: "object", properties: {
            path: { type: "string" }, reason: { type: "string", minLength: 1, maxLength: 4000 },
            disposition: { type: "string", enum: ["open", "needs-finalization", "unknown"] },
          }, required: ["path", "reason"], additionalProperties: false,
        },
        execute: async input => {
          const args = input as { path: string; reason: string; disposition?: string };
          return { content: JSON.stringify(await tracker.note(args.path, args.reason, args.disposition)), metadata: { chaosboxDerived: true } };
        },
      });
      editor.add({
        name: "scratch_explain", options: { namespace: "chaosbox" },
        description: "Retrieve a scratch folder's source-backed purpose, command/session provenance and outstanding finalization work.",
        input: { type: "object", properties: { path: { type: "string" } }, required: ["path"], additionalProperties: false },
        execute: async input => ({ content: JSON.stringify(await tracker.explain((input as { path: string }).path)), metadata: { chaosboxDerived: true } }),
      });
      editor.add({
        name: "scratch_status", options: { namespace: "chaosbox" },
        description: "Report scratch observer coverage, capture failures and unresolved execution counts.",
        input: { type: "object", properties: {}, additionalProperties: false },
        execute: async () => ({ content: JSON.stringify(tracker.status()), metadata: { chaosboxDerived: true } }),
      });
    });
    return async () => { controller.abort(); await observation; await release(); };
  },
});
