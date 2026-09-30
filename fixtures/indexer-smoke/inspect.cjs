// Optional compiler feasibility assertions, using the official SCIP schema.
// Run after the commands in docs/INDEXER_FEASIBILITY.md. Not a production importer.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");

const work = path.resolve(process.argv[2]);
const protobuf = require(path.join(work, "tooling/node_modules/protobufjs"));
const Index = protobuf.loadSync(path.join(work, "scip.proto")).lookupType("scip.Index");
const names = ["rust-default", "rust-extra", "typescript", "typescript-unresolved"];
const indexes = Object.fromEntries(names.map(name => [name, Index.toObject(
  Index.decode(fs.readFileSync(path.join(work, `${name}.scip`))), { defaults: true },
)]));
const all = index => index.documents.flatMap(document => document.occurrences);
const symbols = index => index.documents.flatMap(document => document.symbols);

function aliases(name, file, sourceDirectory) {
  const index = indexes[name];
  const document = index.documents.find(doc => doc.relativePath === file);
  assert.ok(document, `${name}: missing ${file}`);
  const lines = fs.readFileSync(path.join(work, sourceDirectory, file), "utf8").split("\n");
  const rust = name.startsWith("rust");
  // Version-specific compatibility evidence, not a generic encoding guess.
  assert.equal(document.positionEncoding, rust ? 1 : 0);
  if (!rust) assert.equal(index.metadata.toolInfo.version, "0.4.0");
  const tokens = document.occurrences.filter(occ => occ.range.length === 3).map(occ => {
    const [line, start, end] = occ.range;
    const text = rust
      ? Buffer.from(lines[line], "utf8").subarray(start, end).toString("utf8")
      : lines[line].slice(start, end);
    return { ...occ, text, line: lines[line] };
  });
  const call = tokens.find(occ => occ.text === "renamed" && occ.line.includes("value ="));
  const reference = tokens.find(occ => occ.text === "renamed" && occ.line.includes("reference ="));
  const shadow = tokens.find(occ => occ.text === "renamed" && occ.symbol.startsWith("local ")
    && !(occ.symbolRoles & 1));
  assert.ok(call && reference && shadow, `${name}: alias/shadow evidence missing`);
  assert.equal(call.symbol, reference.symbol);
  assert.equal(call.symbolRoles, 0);
  assert.equal(reference.symbolRoles, 0);
  assert.equal(call.syntaxKind, reference.syntaxKind);
  assert.notEqual(call.symbol, shadow.symbol);
  assert.ok(document.occurrences.some(occ => occ.symbol === shadow.symbol && (occ.symbolRoles & 1)));
  const start = call.range[1];
  const byteColumn = rust ? start : Buffer.byteLength(call.line.slice(0, start), "utf8");
  // The test token follows an astral character on the same line.
  if (!rust) assert.equal(byteColumn, start + 2);
  return { call: call.symbol, source_range: call.range, utf8_start_column: byteColumn,
    shadow: shadow.symbol, call_and_value_reference_have_same_roles: true };
}

const rust = indexes["rust-default"];
const extra = indexes["rust-extra"];
const ts = indexes.typescript;
const missing = indexes["typescript-unresolved"];
assert.deepEqual(rust.documents.map(d => d.relativePath).sort(),
  ["helper/src/lib.rs", "src/lib.rs", "src/provider.rs"]);
assert.deepEqual(ts.documents.map(d => d.relativePath).sort(),
  ["src/entry.cts", "src/entry.mts", "src/main.ts", "src/provider.ts"]);
assert.ok(!symbols(rust).some(s => s.symbol.endsWith("configured().")));
assert.ok(symbols(extra).some(s => s.symbol.endsWith("configured().")));
assert.deepEqual(rust.metadata, extra.metadata, "SCIP metadata omits the feature change");
const rustLines = fs.readFileSync(path.join(work, "rust/src/lib.rs"), "utf8").split("\n");
const inactiveBody = rustLines.findIndex(line => line.includes("fn configured")) + 1;
assert.ok(rust.documents.find(d => d.relativePath === "src/lib.rs").occurrences
  .some(o => o.range[0] === inactiveBody && o.symbol.endsWith("provider/answer().")),
  "default index still contains a reference inside the inactive function");
assert.ok(all(rust).some(o => o.symbol === "rust-analyzer cargo helper 0.2.0 dependency()."));
assert.equal(symbols(rust).flatMap(s => s.relationships).length, 0);
assert.equal(symbols(ts).flatMap(s => s.relationships).filter(r => r.isImplementation).length, 2);
assert.ok(symbols(rust).some(s => s.symbol.endsWith("generated().")));
assert.ok(!all(rust).some(o => o.symbol.endsWith("generated().") && (o.symbolRoles & 1)));
assert.equal(all(missing).flatMap(o => o.diagnostics).length, 0);
const missingMain = missing.documents.find(d => d.relativePath === "src/main.ts");
assert.ok(!missingMain.occurrences.some(o => o.symbol.endsWith("/answer().")));
assert.ok(missingMain.occurrences.some(o => o.symbol.startsWith("local ") &&
  !missingMain.occurrences.some(def => def.symbol === o.symbol && (def.symbolRoles & 1))));

const summary = {
  indexes: Object.fromEntries(names.map(name => {
    const index = indexes[name];
    return [name, {
      tool: index.metadata.toolInfo,
      sha256: crypto.createHash("sha256").update(fs.readFileSync(path.join(work, `${name}.scip`))).digest("hex"),
      documents: index.documents.length,
      occurrences: all(index).length,
      symbols: symbols(index).length,
      definition_occurrences: all(index).filter(o => o.symbolRoles & 1).length,
      relationships: symbols(index).flatMap(s => s.relationships).length,
      diagnostics: all(index).flatMap(o => o.diagnostics).length,
    }];
  })),
  rust: aliases("rust-default", "src/lib.rs", "rust"),
  typescript: aliases("typescript", "src/main.ts", "typescript"),
  passed: true,
};
fs.writeFileSync(path.join(work, "summary.json"), JSON.stringify(summary, null, 2) + "\n");
console.log(JSON.stringify(summary, null, 2));
