// SPDX-License-Identifier: Apache-2.0
// The specialized task actions that submit an expected version and a target of
// their own instead of the whole task. Save keeps its own attempt in TaskEditor.
export enum SpecializedAction {
  SetProposal = "setProposal",
  RestorePrompt = "restorePrompt",
}

// The complete logical request. Two submissions share a request ID only when
// every field matches, so a changed action, task, expected base, proposal target
// or prompt source always receives a fresh ID instead of REQUEST_ID_REUSED.
export type SpecializedRequest =
  | {
      action: SpecializedAction.SetProposal;
      id: string;
      expectedVersionId: string | undefined;
      proposal: number;
    }
  | {
      action: SpecializedAction.RestorePrompt;
      id: string;
      expectedVersionId: string | undefined;
      sourceVersionId: string;
    };

// Keeps the request identity of one unresolved specialized action. A rejected
// reply, including one lost after the store committed the action, leaves the
// attempt in place so an unchanged explicit retry replays the committed result
// instead of creating a second branch from the same base. A resolved reply ends
// the attempt, so the next independent action gets a new ID.
export class RequestAttempt {
  private pending?: { key: string; requestId: string };

  constructor(private readonly allocate: () => string) {}

  async submit<T>(
    request: SpecializedRequest,
    send: (requestId: string) => Promise<T>,
  ): Promise<T> {
    const key = JSON.stringify(request);
    if (this.pending?.key !== key)
      this.pending = { key, requestId: this.allocate() };
    const attempt = this.pending;
    const reply = await send(attempt.requestId);
    // A newer attempt must not be cleared by an older reply.
    if (this.pending === attempt) this.pending = undefined;
    return reply;
  }
}
