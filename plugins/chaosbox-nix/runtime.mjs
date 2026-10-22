import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { resolve } from "node:path";

// Read-only queries cannot select a mutation. Additions compose the native
// shell leaf so execution-time shell authorization remains authoritative.
export function readArgs(operation, input = {}) {
  switch (operation) {
    case "capabilities": return ["nix", "capabilities"];
    case "context":
      if (typeof input.repo !== "string" || !input.repo.trim() || typeof input.query !== "string" || !input.query.trim()) throw new Error("repository and query required");
      return ["nix", "context", "--repo", input.repo, "--limit", "3", "--", input.query];
    case "evidence":
      if (typeof input.repo !== "string" || !input.repo.trim() || typeof input.id !== "string" || !input.id.trim()) throw new Error("repository and operation identity required");
      return ["nix", "evidence", "--repo", input.repo, "--", input.id];
    case "query": {
      const offset = input.offset ?? 0;
      if (!Number.isInteger(offset) || offset < 0 || offset > 1_000_000) throw new Error("invalid offset");
      return ["nix", "query", "--limit", "20", "--offset", String(offset), ...(input.path ? ["--path", input.path] : [])];
    }
    default: throw new Error("unsupported read operation");
  }
}

export function runRead(binary, operation, input, signal) {
  if (typeof binary !== "string" || !binary.startsWith("/")) throw new Error("absolute Chaosbox binary required");
  const args = readArgs(operation, input);
  return new Promise((resolve, reject) => {
    execFile(binary, args, { timeout: 5000, maxBuffer: 4 * 1024 * 1024, signal }, (error, stdout) => {
      if (error) return reject(error);
      try { resolve(JSON.parse(stdout)); } catch (error) { reject(error); }
    });
  });
}

export function verify(packet, options) {
  if (packet?.version !== 1 || packet.scope !== options.scope || packet.host !== options.host || packet.store !== (options.store ?? "daemon")) throw new Error("Nix evidence namespace/version mismatch");
  return packet;
}

export function queryTerms(messages) {
  const latest = [...messages].reverse().find(message => message.role === "user");
  const text = typeof latest?.content === "string" ? latest.content : (latest?.content ?? []).filter(part => part.type === "text").map(part => part.text).join(" ");
  if (!/\bnix\b|\/nix\/store\//i.test(text)) return [];
  const paths = text.match(/\/nix\/store\/[a-z0-9]{32}-[A-Za-z0-9+._?=-]+/g);
  if (paths?.length) return [...new Set(paths)].slice(0, 3);
  return [...new Set(text.toLowerCase().match(/[a-z][a-z-]{5,}/g) ?? [])].filter(word => !["please", "through", "chaosbox", "reason", "should"].includes(word)).slice(0, 3);
}

export function injection(records, maxChars = 12000) {
  const selected = [];
  let omitted = 0;
  for (const record of records) {
    if (JSON.stringify([...selected, record]).length > maxChars) { omitted++; continue; }
    selected.push(record);
  }
  return { records: selected, omitted_records: omitted };
}

export function addition(binary, input, directory, context) {
  if (typeof binary !== "string" || !binary.startsWith("/") || typeof directory !== "string" || !directory.startsWith("/")) throw new Error("absolute facade and session directory required");
  if (!/^\/[A-Za-z0-9/._+-]+$/.test(binary)) throw new Error("shell facade path must have a literal safe command spelling");
  if (typeof input.reason !== "string" || !input.reason.trim() || Buffer.byteLength(input.reason) > 4000 || input.reason.includes("\0")) throw new Error("nonblank reason of at most 4000 UTF-8 bytes required");
  if (typeof input.path !== "string" || !input.path || input.path.includes("\0")) throw new Error("input path required");
  if (input.mode !== undefined && !["nar", "flat"].includes(input.mode)) throw new Error("unsupported addressing mode");
  for (const key of ["sessionID", "messageID", "id"]) if (typeof context[key] !== "string" || !context[key]) throw new Error(`native ${key} required`);
  const id = `opencode:${createHash("sha256").update(JSON.stringify([context.sessionID, context.messageID, context.id])).digest("hex")}`;
  const argv = [binary, "nix", "add", "--reason", input.reason, "--repo", directory, "--id", id, "--session", context.sessionID, "--message", context.messageID, "--tool-call", context.id, "--mode", input.mode ?? "nar", "--", resolve(directory, input.path)];
  const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
  // Native permissions match source-shaped resources. Keep the fixed command
  // prefix literal, while every caller/native value remains shell-quoted.
  return { command: `${binary} nix add ${argv.slice(3).map(quote).join(" ")}`, workdir: directory, timeout: 260000, background: false };
}

export async function runAddition(leaf, binary, input, directory, context) {
  const request = addition(binary, input, directory, context);
  if (!leaf || leaf.id !== "shell" || typeof leaf.execute !== "function") throw new Error("native OpenCode shell leaf unavailable; mutation refused");
  // Do not spawn directly: the native leaf validates directories, checks parsed
  // command resources with permission.assert, and owns interruption/job cleanup.
  const result = await leaf.execute(request, context);
  // The facade tool returns model evidence, not the native shell's machine
  // output schema. Returning its undeclared `output` would fail Core settlement.
  return { content: result.content, metadata: result.metadata };
}
