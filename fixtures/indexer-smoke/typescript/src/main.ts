import { answer as renamed, Worker } from "@local/provider.js";

export function consume(): number {
  const label = "🦀"; const value = renamed();
  const reference = renamed;
  const shadowed = (() => { const renamed = () => 7; return renamed(); })();
  return value + reference() + shadowed + new Worker().run() + label.length;
}

// function comment_fake() {}
export const text = "function string_fake() {}";
