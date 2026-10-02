import { watch } from "node:fs";
import { lstat, opendir, mkdtemp } from "node:fs/promises";
import { hostname } from "node:os";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { randomUUID } from "node:crypto";
import { subprocess } from "../chaosbox-compaction/runtime.mjs";

export { subprocess };

const below = (path, root) => path !== root && path.startsWith(`${root}${sep}`);
const keyOf = event => `${event.sessionID}:${event.messageID}:${event.id}`;
const fsIdentity = meta => ({ dev: String(meta.dev), ino: String(meta.ino), birth_ns: String(meta.birthtimeNs) });
const instances = new Map();

/** All location-scoped plugin instances share one observer in the server process. */
export async function acquireTracker(options) {
  const key = JSON.stringify([options.work, options.scope, options.root, options.host, options.chaosboxBin, options.liveAssessment, options.maxRequests, options.maxInputTokens]);
  let shared = instances.get(key);
  if (!shared) {
    shared = { users: 0, tracker: createTracker(options) };
    instances.set(key, shared);
    shared.tracker.catch(() => instances.delete(key));
  }
  shared.users++;
  const tracker = await shared.tracker;
  return { tracker, async release() {
    if (--shared.users === 0) { instances.delete(key); await tracker.close(); }
  } };
}

/** Exact native text projections; omit reasoning, provider state and tool bodies. */
export function purposeSources(records) {
  const eligible = records.filter(record => ["user", "assistant"].includes(record.type) && !record.metadata?.chaosboxDerived);
  const hasText = record => (typeof record.text === "string" && record.text.trim())
    || (record.prompt?.parts ?? record.content ?? record.parts ?? []).some(part => part.type === "text" && typeof part.text === "string" && part.text.trim());
  const latest = [eligible.findLast(record => record.type === "user" && hasText(record)), eligible.findLast(record => record.type === "assistant" && hasText(record))].filter(Boolean);
  const sources = [];
  let omitted = 0;
  for (const original of latest) {
    const key = original.type === "user" && Array.isArray(original.prompt?.parts) ? "prompt" : Array.isArray(original.content) ? "content" : "parts";
    const record = { id: original.id, type: original.type, time: original.time };
    if (record.type === "user" && typeof original.text === "string") record.text = original.text;
    const parts = key === "prompt" ? original.prompt.parts : original[key];
    const projection = Array.isArray(parts) ? parts.map(part => part.type === "text" ? { type: "text", text: part.text } : { type: part.type }) : [];
    if (key === "prompt") record.prompt = { parts: projection };
    else if (Array.isArray(parts)) record[key] = projection;
    if (Buffer.byteLength(JSON.stringify(record)) > 128 * 1024) {
      omitted += Number(Boolean(record.text)) + projection.filter(p => p.type === "text").length;
      continue;
    }
    if (typeof record.text === "string" && record.text.trim()) sources.push({ record, pointer: "/text" });
    for (const [index, part] of projection.entries()) {
      if (part.type === "text" && typeof part.text === "string" && part.text.trim()) sources.push({ record, pointer: key === "prompt" ? `/prompt/parts/${index}/text` : `/${key}/${index}/text` });
    }
  }
  const retained = sources.slice(-8);
  Object.defineProperty(retained, "omitted", { value: omitted + Math.max(0, sources.length - retained.length) });
  return retained;
}

/** Bounded work-allocation observation. Generated descendants are aggregated. */
export async function createTracker(options, run = subprocess) {
  const root = options.root ?? "/data/scratch/tmp/opencode";
  const work = options.work;
  const scope = options.scope;
  const host = options.host ?? hostname();
  if (![root, work].every(path => typeof path === "string" && isAbsolute(path) && resolve(path) === path)
    || root === "/" || root === work || below(work, root) || below(root, work)) throw new Error("Scratch root and durable custody must be separate absolute directories");
  if (typeof scope !== "string" || !/^private:.+/.test(scope)) throw new Error("Private scratch scope is required");
  const binary = options.chaosboxBin ?? "chaosbox";
  const baseArgs = ["scratch", "--work", work, "--scope", scope, "--host", host, "--root", root];
  const maxEntries = options.maxEntries ?? 10_000;
  const maxWatches = options.maxWatches ?? 4096;
  if (![maxEntries, maxWatches].every(n => Number.isInteger(n) && n >= 1 && n <= 100_000)) throw new Error("Invalid scratch observation budget");
  const observer = randomUUID();
  let sequence = 0;
  let closed = false;
  let chain = Promise.resolve();
  let healthy = true;
  let initialized = false;
  const state = { degraded: false, lastError: "", pending: 0, allocations: 0, observer, root, host };
  state.assessment = { enabled: options.liveAssessment === true, running: false, pending: false, lastError: "" };
  let worker;
  let assessmentTimer;
  const sessionCalls = new Map();
  function wake() {
    if (closed || options.liveAssessment !== true) return;
    state.assessment.pending = true;
    if (worker || assessmentTimer) return;
    assessmentTimer = setTimeout(() => {
      assessmentTimer = undefined;
      state.assessment.pending = false;
      state.assessment.running = true;
      worker = run(binary,[...baseArgs,"assess","--privacy-reviewed","--max-requests",String(options.maxRequests ?? 1000),
        "--max-input-tokens",String(options.maxInputTokens ?? 10_000_000)],undefined,120_000)
        .then(result => { state.assessment.lastError = ""; if (result?.remaining) state.assessment.pending = true; })
        .catch(error => { state.assessment.lastError = String(error).slice(0,1000); })
        .finally(() => { worker = undefined; state.assessment.running = false; if (state.assessment.pending) wake(); });
    }, options.assessmentDelayMs ?? 1000);
  }
  const active = new Map();
  const seenTools = new Set();
  const shells = new Map();
  const seenShells = new Set();
  const earlyExits = new Map();
  const known = new Map();
  const watchers = new Map();
  const watchOwners = new Map();
  const pending = [];
  const dirty = new Map();
  const nextID = () => `${observer}:${++sequence}`;
  const enqueue = event => {
    if (pending.length >= 1024) {
      healthy = false;
      state.degraded = true;
      throw new Error("Scratch event queue exhausted; coverage is incomplete");
    }
    pending.push({ id: nextID(), ...event });
    state.pending = pending.length;
  };
  const coverage = detail => enqueue({ kind: "coverage", observer, healthy, detail: detail.slice(0, 1000) });
  const degrade = error => {
    healthy = false;
    state.degraded = true;
    state.lastError = String(error).slice(0, 1000);
  };
  const exclusive = fn => {
    const task = chain.then(fn);
    chain = task.catch(error => { degrade(error); });
    return task;
  };
  async function commit() {
    while (pending.length) {
      const events = [];
      let bytes = 1024;
      for (const event of pending.slice(0, 128)) {
        const size = Buffer.byteLength(JSON.stringify(event)) + 1;
        if (bytes + size > 4 * 1024 * 1024) break;
        events.push(event);
        bytes += size;
      }
      if (!events.length) throw new Error("Scratch event exceeds wire budget");
      await run(binary, [...baseArgs, "record"], { version: 1, scope, host, root, events }, 5000);
      if (events.some(e => ["end","context","link","annotation"].includes(e.kind) || (e.kind === "observe" && e.owners?.length))) wake();
      pending.splice(0, events.length);
      state.pending = pending.length;
    }
    state.degraded = !healthy;
    if (healthy) state.lastError = "";
  }
  async function metadata(path) {
    // Check every component before observing or installing a watcher.
    let cursor = sep;
    const rootMeta = await lstat(root, { bigint: true });
    for (const part of path.split(sep).filter(Boolean)) {
      cursor = join(cursor, part);
      const meta = await lstat(cursor, { bigint: true });
      if (meta.isSymbolicLink()) throw new Error("Scratch symlink component refused");
      if ((cursor === root || below(cursor, root)) && meta.dev !== rootMeta.dev) throw new Error("Scratch device boundary refused");
      if (cursor === path) {
        if (!meta.isDirectory() && !meta.isFile()) throw new Error("Scratch special file refused");
        return meta;
      }
      if (!meta.isDirectory()) throw new Error("Scratch ancestor is not a directory");
    }
    throw new Error("Invalid scratch path");
  }
  function mark(path) {
    if (closed) return;
    if (dirty.size >= maxEntries && !dirty.has(path)) { degrade(new Error("Scratch activity budget exhausted")); return; }
    const owners = dirty.get(path) ?? new Set();
    for (const id of active.keys()) owners.add(id);
    dirty.set(path, owners);
  }
  function addWatch(path, isRoot = false, allocation = path) {
    if (watchers.has(path)) return;
    if (watchers.size >= maxWatches) { degrade(new Error("Scratch watch budget exhausted")); return; }
    const watcher = watch(path, { encoding: "buffer", persistent: false }, (_event, filename) => {
      if (!filename) { degrade(new Error("Scratch watcher omitted a filename")); return; }
      const name = filename.toString("utf8");
      if (!Buffer.from(name, "utf8").equals(filename)) { degrade(new Error("Non-UTF-8 scratch name requires preservation review")); return; }
      if (isRoot) {
        if (name === ".doty-quarantine") return;
        mark(join(root, name));
      } else {
        mark(allocation); // aggregate generated descendants under their work folder
      }
    });
    watcher.on("error", degrade);
    watchers.set(path, watcher);
    watchOwners.set(path, allocation);
  }
  async function watchTree(path, allocation = path) {
    if (watchers.has(path) && watchOwners.get(path) !== allocation) return;
    addWatch(path, false, allocation);
    const directory = await opendir(path, { encoding: "buffer" });
    let scanned = 0;
    for await (const entry of directory) {
      if (++scanned > maxEntries) { degrade(new Error("Scratch subtree inventory budget exhausted")); break; }
      if (!entry.isDirectory()) continue;
      if (watchers.size >= maxWatches) { degrade(new Error("Scratch recursive watch budget exhausted")); break; }
      const name = entry.name.toString("utf8");
      if (!Buffer.from(name).equals(entry.name)) { degrade(new Error("Non-UTF-8 scratch directory")); continue; }
      const child = join(path, name);
      try {
        const meta = await metadata(child);
        if (meta.isDirectory()) await watchTree(child, allocation);
      } catch (error) { if (error.code !== "ENOENT") degrade(error); }
    }
  }
  async function observe(path, owners = new Set(), activity = false) {
    if (!below(path, root)) throw new Error("Scratch path outside the configured root");
    try {
      const meta = await metadata(path);
      const identity = fsIdentity(meta);
      const previous = known.get(path);
      const changed = !previous || JSON.stringify(previous.identity) !== JSON.stringify(identity);
      if (changed || activity || previous.mtime !== String(meta.mtimeNs)) {
        if (owners.size > 64) degrade(new Error("Scratch possible-owner budget exhausted"));
        enqueue({ kind: "observe", path, identity, present: true, activity: activity || Boolean(previous), owners: [...owners].slice(0, 64) });
      }
      known.set(path, { identity, mtime: String(meta.mtimeNs) });
      if (changed) {
        for (const [watched, owner] of watchOwners) if (owner === path) {
          watchers.get(watched)?.close(); watchers.delete(watched); watchOwners.delete(watched);
        }
      }
      if (meta.isDirectory() && (changed || activity || !watchers.has(path))) await watchTree(path);
    } catch (error) {
      if (error.code === "ENOENT") {
        const previous = known.get(path);
        if (previous) enqueue({ kind: "observe", path, identity: previous.identity, present: false, activity: false, owners: [] });
        else if (activity) enqueue({ kind: "coverage", observer, healthy: false,
          detail: `Filesystem activity vanished before identity inspection: ${JSON.stringify({ path, possibleInvocations:[...owners] })}`.slice(0,1000) });
        known.delete(path);
        for (const [watched, owner] of watchOwners) if (owner === path) {
          watchers.get(watched)?.close(); watchers.delete(watched); watchOwners.delete(watched);
        }
      } else {
        degrade(error);
      }
    }
    state.allocations = known.size;
  }
  async function flushDirty() {
    const changes = [...dirty];
    dirty.clear();
    for (const [path, owners] of changes) await observe(path, owners, true);
  }
  async function reconcile() {
    await flushDirty();
    const seen = new Set();
    const directory = await opendir(root, { encoding: "buffer" });
    let count = 0;
    for await (const entry of directory) {
      if (++count > maxEntries) { degrade(new Error("Scratch inventory budget exhausted")); break; }
      const name = entry.name.toString("utf8");
      if (!Buffer.from(name).equals(entry.name)) { degrade(new Error("Non-UTF-8 scratch inventory name")); continue; }
      if (name === ".doty-quarantine") continue;
      const path = join(root, name);
      seen.add(path);
      await observe(path, new Set(active.keys()), !initialized);
      if (pending.length >= 128) await commit();
    }
    // Explicitly allocated nested work folders are watched and reconciled too.
    for (const path of [...known.keys()]) {
      if (relative(root, path).includes(sep) || !seen.has(path)) await observe(path, new Set(active.keys()));
    }
    coverage(healthy ? "Work-folder creation, direct activity and tool-use observation active; generated descendants aggregated" : state.lastError);
    await commit();
    initialized = true;
  }

  const rootMeta = await metadata(root);
  if (!rootMeta.isDirectory() || rootMeta.isSymbolicLink()) throw new Error("Scratch root must be a real directory");
  addWatch(root, true); // install before the initial inventory, so creation cannot fall between them
  enqueue({ kind: "coverage", observer, healthy: false, detail: "Observer startup/restart: prior offline activity cannot be proven; existing releases require review" });
  try { await exclusive(reconcile); }
  catch (error) { for (const watcher of watchers.values()) watcher.close(); throw error; }
  wake(); // durable pending work survives server restarts
  const timer = setInterval(() => {
    if (!closed) void exclusive(reconcile).catch(() => {});
  }, options.intervalMs ?? 30_000);
  timer.unref();
  const flushTimer = setInterval(() => {
    if (!closed && dirty.size) void exclusive(async () => { await flushDirty(); await commit(); }).catch(() => {});
  }, 250);
  flushTimer.unref();

  async function finishShell(data) {
    const invocation = shells.get(data.id);
    await reconcile();
    enqueue({ kind: "end", invocation, outcome: data.status === "exited" ? "exited" : "interrupted", exit: data.exit ?? null });
    active.delete(invocation);
    shells.delete(data.id);
    await reconcile();
  }

  function remember(session, invocation) {
    const calls = sessionCalls.get(session) ?? new Set();
    calls.add(invocation);
    if (calls.size > 8) calls.delete(calls.values().next().value);
    sessionCalls.set(session,calls);
    if (sessionCalls.size > maxEntries) sessionCalls.delete(sessionCalls.keys().next().value);
  }

  return {
    hasSession: session => sessionCalls.has(session),
    status: () => ({ ...state, assessment: {...state.assessment}, active: active.size }),
    reconcile: () => exclusive(reconcile),
    before: (event, repo, cwd, records) => exclusive(async () => {
      if (event.tool.startsWith("chaosbox_")) return;
      await reconcile();
      const invocation = `tool:${keyOf(event)}`;
      if (seenTools.has(invocation)) return;
      const command = typeof event.input?.command === "string" ? event.input.command : JSON.stringify(event.input ?? {});
      const sources = purposeSources(records);
      enqueue({ kind: "begin", invocation, session: event.sessionID, message: event.messageID,
        tool: event.tool, repo, command, cwd: event.input?.workdir ?? cwd, sources, sources_omitted: sources.omitted });
      seenTools.add(invocation);
      if (seenTools.size > maxEntries) seenTools.delete(seenTools.values().next().value);
      active.set(invocation, { session: event.sessionID, repo, command, cwd: event.input?.workdir ?? cwd, sources });
      remember(event.sessionID,invocation);
      // References to an existing allocation are evidence of use, even without mutation.
      // Doty's inspection/removal commands name targets without resuming their work.
      const cleanup = /^\s*(?:[^\s;&|`$]*\/)?doty\s+(?:analyze|rm|purge|restore|run|status)\b[^;&|`$\n]*$/.test(command);
      for (const path of known.keys()) if (!cleanup && command.includes(path)) await observe(path, new Set([invocation]), true);
      await commit();
    }),
    after: event => exclusive(async () => {
      const invocation = `tool:${keyOf(event)}`;
      if (!active.has(invocation)) return;
      await reconcile();
      enqueue({ kind: "end", invocation, outcome: event.status === "error" ? "error" : "exited", exit: event.result?.metadata?.exit ?? event.result?.output?.exit ?? null });
      active.delete(invocation); // native shell ownership persists independently for background jobs
      await reconcile();
    }),
    event: event => exclusive(async () => {
      if (event.type === "shell.created") {
        const info = event.data.info;
        const session = info.metadata?.sessionID;
        if (!session || seenShells.has(info.id)) return;
        seenShells.add(info.id);
        if (seenShells.size > maxEntries) seenShells.delete(seenShells.values().next().value);
        const candidates = [...active.entries()].filter(([id,value]) => id.startsWith("tool:") && value.session === session && value.command === info.command);
        const candidate = candidates.length === 1 ? candidates[0][1] : undefined;
        const invocation = `shell:${info.id}`;
        enqueue({ kind: "begin", invocation, session, message: "", tool: "native-shell", repo: candidate?.repo ?? "unassociated",
          command: info.command, cwd: info.cwd, sources: candidate?.sources ?? [], sources_omitted: candidate?.sources?.omitted ?? 0 });
        active.set(invocation, { session, repo: candidate?.repo ?? "unassociated", command: info.command, cwd: info.cwd, sources: candidate?.sources ?? [] });
        remember(session,invocation);
        shells.set(info.id, invocation);
        await commit();
        if (earlyExits.has(info.id)) {
          const data = earlyExits.get(info.id);
          earlyExits.delete(info.id);
          await finishShell(data);
        }
      } else if (event.type === "shell.exited") {
        if (shells.has(event.data.id)) await finishShell(event.data);
        else {
          if (earlyExits.size >= maxEntries) earlyExits.delete(earlyExits.keys().next().value);
          earlyExits.set(event.data.id, event.data);
        }
      }
    }),
    settled: (session, records) => exclusive(async () => {
      const sources = purposeSources(records);
      for (const invocation of sessionCalls.get(session) ?? []) enqueue({kind:"context",invocation,sources});
      await commit();
    }),
    async workspace(purpose, event, repo, cwd, records) {
      if (typeof purpose !== "string" || !purpose.trim() || purpose.length > 4000) throw new Error("Workspace purpose required");
      return exclusive(async () => {
        const invocation = `workspace:${keyOf(event)}`;
        const sources = purposeSources(records);
        enqueue({kind:"begin",invocation,session:event.sessionID,message:event.messageID,tool:"scratch-workspace",repo,
          command:JSON.stringify({purpose}),cwd,sources,sources_omitted:sources.omitted});
        await commit(); // custody precedes creation
        const path = await mkdtemp(join(root,"workspace-"));
        try {
          await observe(path,new Set([invocation]),true);
          enqueue({kind:"annotation",path,identity:known.get(path).identity,disposition:"open",reason:purpose});
          enqueue({kind:"end",invocation,outcome:"exited",exit:0});
          remember(event.sessionID,invocation);
          await commit();
          return {path,purpose,session:event.sessionID,invocation};
        } catch (error) { degrade(error); throw error; }
      });
    },
    async link(path, category, reference, description, context) {
      await exclusive(async () => { await observe(path); await commit(); });
      const source = context ? ["--session",context.sessionID,"--message",context.messageID] : [];
      const result = await run(binary,[...baseArgs,"link",path,"--category",category,"--reference",reference,"--description",description,...source],undefined,5000);
      wake();
      return result;
    },
    async note(path, reason, disposition = "needs-finalization") {
      if (!below(path, root) || resolve(path) !== path || !["open", "needs-finalization", "unknown"].includes(disposition)) throw new Error("Invalid scratch annotation");
      await exclusive(async () => { await observe(path); await commit(); });
      const result = await run(binary, [...baseArgs, "annotate", path, "--reason", reason, "--disposition", disposition], undefined, 5000);
      wake();
      return result;
    },
    explain: path => run(binary, [...baseArgs, "explain", path], undefined, 5000),
    failure: error => exclusive(async () => { degrade(error); coverage(state.lastError); await commit(); }),
    async close() {
      closed = true;
      clearInterval(timer);
      clearInterval(flushTimer);
      clearTimeout(assessmentTimer);
      for (const watcher of watchers.values()) watcher.close();
      await exclusive(async () => {
        await flushDirty();
        for (const invocation of active.keys()) enqueue({ kind: "end", invocation, outcome: "unknown", exit: null });
        healthy = false;
        coverage("Observer unloaded; unfinished invocations remain unresolved");
        await commit();
      });
      await worker;
    },
  };
}
