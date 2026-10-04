import type {
  SessionCompactionPlan,
  SessionContext,
} from "@opencode/plugin/promise/session";
import type { Result, Error } from "@opencode/plugin/promise/tool";

/** Typed adapter seam; command responses are checked by the runtime before use. */
export function subprocess(
  binary: string,
  args: readonly string[],
  input?: unknown,
  timeout?: number,
): Promise<unknown>;

type LoadContext = (sessionID: string) => Promise<readonly unknown[]>;
type Settlement = {
  readonly sessionID: string;
  readonly messageID: string;
  readonly id: string;
  readonly tool: string;
  readonly input: unknown;
} & (
  | { readonly status: "completed"; result: Result }
  | { readonly status: "error"; error: Error }
);

export function coordinator(
  options: Record<string, unknown>,
  run: typeof subprocess,
): {
  wake(): void;
  settled(sessionID: string, records: readonly unknown[]): Promise<void>;
  observe(
    events: AsyncIterable<{ type: string; data: unknown }>,
    load: LoadContext,
  ): Promise<void>;
  compact(event: SessionCompactionPlan): Promise<void>;
  tool(event: Settlement): Promise<void>;
  context(event: SessionContext, load?: LoadContext): Promise<void>;
  archive(input: unknown): Promise<unknown>;
  retrieve(input: unknown): Promise<unknown>;
  status(): Promise<unknown>;
};
