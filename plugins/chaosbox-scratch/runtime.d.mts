type Invocation = {
  readonly sessionID: string;
  readonly messageID: string;
  readonly id: string;
  readonly tool: string;
  readonly input?: unknown;
};

type Tracker = {
  hasSession(session: string): boolean;
  status(): unknown;
  before(event: Invocation, repo: string, cwd: string, records: readonly unknown[]): Promise<void>;
  after(event: Invocation & { readonly status: "completed" | "error" }): Promise<void>;
  event(event: { readonly type: string; readonly data: unknown }): Promise<void>;
  settled(session: string, records: readonly unknown[]): Promise<void>;
  workspace(purpose: string, event: Invocation, repo: string, cwd: string, records: readonly unknown[]): Promise<unknown>;
  link(path: string, category: string, reference: string, description: string, context?: Pick<Invocation,"sessionID"|"messageID">): Promise<unknown>;
  note(path: string, reason: string, disposition?: string): Promise<unknown>;
  explain(path: string): Promise<unknown>;
  reconcile(): Promise<void>;
  failure(error: unknown): Promise<void>;
  close(): Promise<void>;
};

export function createTracker(options: Record<string, unknown>): Promise<Tracker>;
export function acquireTracker(options: Record<string, unknown>): Promise<{ tracker: Tracker; release(): Promise<void> }>;
