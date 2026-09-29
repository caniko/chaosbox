export function answer(): number { return 42; }

export interface Work { run(): number; }

export class Worker implements Work {
  run(): number { return answer(); }
}
