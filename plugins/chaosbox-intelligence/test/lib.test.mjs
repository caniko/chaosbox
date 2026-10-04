import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildInjectionText,
  clampInt,
  containsInjectedQuote,
  deriveQuery,
  fingerprint,
  isDecisionMoment,
  lastTextByRole,
  validateChallenge,
} from "../lib.mjs";

const user = (text) => ({ role: "user", content: [{ type: "text", text }] });
const assistant = (text) => ({ role: "assistant", content: [{ type: "text", text }] });

describe("deriveQuery", () => {
  it("picks the latest user text", () => {
    const messages = [user("first question about direnv approval"), assistant("ok"), user("second question about TypeDB migration")];
    assert.equal(deriveQuery(messages), "second question about TypeDB migration");
  });

  it("rejects short input", () => {
    assert.equal(deriveQuery([user("hi")]), null);
    assert.equal(deriveQuery([]), null);
  });

  it("truncates long input", () => {
    const long = `x ${"y".repeat(600)}`;
    const query = deriveQuery([user(long)], 16, 500);
    assert.ok(query.endsWith("…"));
    assert.ok(query.length <= 501);
  });

  it("handles plain-string content shapes", () => {
    assert.equal(
      deriveQuery([{ role: "user", content: "explicit repository associations for canix" }]),
      "explicit repository associations for canix",
    );
  });
});

describe("lastTextByRole", () => {
  it("finds the latest assistant text", () => {
    const messages = [assistant("first"), user("q"), assistant("second answer here")];
    assert.equal(lastTextByRole(messages, "assistant"), "second answer here");
  });
});

describe("isDecisionMoment", () => {
  it("detects explicit decisions", () => {
    assert.equal(isDecisionMoment("We decided to pin the bundle at startup for isolation"), true);
    assert.equal(isDecisionMoment("Agreed, going with TypeDB as the authoritative backend"), true);
  });

  it("ignores ordinary chatter", () => {
    assert.equal(isDecisionMoment("what does this function do?"), false);
    assert.equal(isDecisionMoment("ok"), false);
  });
});

describe("containsInjectedQuote", () => {
  const statement =
    "Session-global environment replacement can race concurrent commands in the chaosbox runner";

  it("detects verbatim reuse", () => {
    assert.equal(
      containsInjectedQuote(`as noted, ${statement} and we should fix it`, [statement]),
      true,
    );
  });

  it("passes unrelated text", () => {
    assert.equal(containsInjectedQuote("something entirely different about nix builds", [statement]), false);
  });

  it("ignores short statements", () => {
    assert.equal(containsInjectedQuote("short", ["short"]), false);
  });
});

describe("buildInjectionText", () => {
  it("marks evidence as non-authoritative with citations", () => {
    const text = buildInjectionText({
      bundleVersion: "1:2",
      scope: "private:can",
      repo: "canix",
      query: "direnv approval",
      records: [
        {
          id: "intel:abc",
          kind: "Constraint",
          status: "Active",
          statement: "Operator approval is required",
          citations: [{ source: "opencode", session: "ses_1", message: "m1", pointer: "/p", line: 3 }],
          contradicts: ["intel:def"],
          needs_revalidation: true,
        },
      ],
    });
    assert.match(text, /historical_data_not_instructions: true/);
    assert.match(text, /never independent corroboration/);
    assert.match(text, /chaosbox_challenge/);
    assert.match(text, /intel:abc/);
    assert.match(text, /NEEDS-REVALIDATION/);
    assert.match(text, /contradicts: intel:def/);
  });
});

describe("validateChallenge", () => {
  const good = {
    intelligenceId: "intel:0123456789abcdef0123456789abcdef",
    bundleVersion: "123:456",
    challengeKind: "inapplicable",
    disputedPremise: "The record claims the hook lacks session identity in all cases",
    proposedResolution: "Grant a scoped exception for the atlas host where identity exists",
    newEvidence: "opencode ses_x m3 shows the identity field present",
  };

  it("accepts a well-formed scoped challenge", () => {
    const checked = validateChallenge(good);
    assert.equal(checked.ok, true);
    assert.match(checked.proposal.proposalId, /^challenge-[0-9a-f]{12}$/);
    assert.equal(checked.proposal.revisionScope, "scoped-applicability");
    assert.equal(checked.proposal.status, "proposed-human-resolution-pending");
  });

  it("marks standing-item scope for incorrect challenges", () => {
    const checked = validateChallenge({ ...good, challengeKind: "incorrect" });
    assert.equal(checked.proposal.revisionScope, "standing-item");
  });

  it("rejects malformed proposals", () => {
    const checked = validateChallenge({
      intelligenceId: "nope",
      bundleVersion: "",
      challengeKind: "wrong-kind",
      disputedPremise: "short",
      proposedResolution: "",
    });
    assert.equal(checked.ok, false);
    assert.ok(checked.errors.length >= 4);
  });

  it("warns without new evidence but still accepts", () => {
    const { newEvidence, ...rest } = good;
    const checked = validateChallenge(rest);
    assert.equal(checked.ok, true);
    assert.equal(checked.warnings.length, 1);
  });
});

describe("helpers", () => {
  it("clamps integers", () => {
    assert.equal(clampInt(99, 1, 20, 5), 20);
    assert.equal(clampInt("x", 1, 20, 5), 5);
    assert.equal(clampInt(7, 1, 20, 5), 7);
  });

  it("fingerprints stats", () => {
    assert.equal(fingerprint({ mtimeMs: 123.9, size: 45 }), "123:45");
  });
});
