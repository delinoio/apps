// SPDX-License-Identifier: Apache-2.0
import { LaneService, ResultKind } from "@repo/protos/lane/v1/lane_pb";
import { type LaneAdapter, newId } from "./adapter.ts";

export interface RepositoryDraft {
  id: string;
  base?: string;
  url: string;
  name: string;
}

export enum RepositorySaveOutcome {
  // The mutation was applied or replayed.
  Saved = "saved",
  // The server kept the submitted version as a sibling of a newer head.
  Conflict = "conflict",
  // The request failed or its response was lost; the outcome on the server is unknown.
  Failed = "failed",
  // There was nothing to submit: no editor, or its save was already pending.
  Ignored = "ignored",
}

export interface RepositorySaveResult {
  outcome: RepositorySaveOutcome;
  // True only while the editor that submitted the save is still the active editor.
  // A save that finishes after its editor closed (or after a newer editor opened,
  // including one for the same repository) must not change or report into that editor.
  current: boolean;
  error?: unknown;
}

export const repositoryConflictMessage =
  "Repository versions conflict. Your input is retained. Close this editor to compare and resolve the saved alternatives.";

// Owns the lifetime of the repository form and its save attempts. Every opened editor is
// a new lifetime, even for the same repository ID, and closing ends the lifetime. Closing
// is not cancellation: a submitted mutation still completes, but only its own lifetime may
// react to the outcome.
export class RepositoryEditor {
  #draft?: RepositoryDraft;
  #lifetime = 0;
  // Lifetime of the save currently in flight. Only one save runs per lifetime.
  #pending?: number;
  // Keeps the request identity across an unchanged retry, including response loss.
  #attempt?: { lifetime: number; key: string; requestId: string };

  readonly #changed: () => void;
  readonly #requestId: () => string;

  constructor(changed: () => void, requestId: () => string = newId) {
    this.#changed = changed;
    this.#requestId = requestId;
  }

  get draft(): RepositoryDraft | undefined {
    return this.#draft;
  }

  // True while the active editor's own save is pending; its input is locked.
  get saving(): boolean {
    return this.#draft !== undefined && this.#pending === this.#lifetime;
  }

  open(draft: RepositoryDraft): void {
    this.#lifetime++;
    this.#attempt = undefined;
    this.#draft = { ...draft };
    this.#changed();
  }

  close(): void {
    this.#lifetime++;
    this.#attempt = undefined;
    this.#draft = undefined;
    this.#changed();
  }

  edit(patch: Partial<Pick<RepositoryDraft, "url" | "name">>): void {
    // Input accepted during a pending save would be discarded by its completion.
    if (!this.#draft || this.saving) return;
    this.#draft = { ...this.#draft, ...patch };
    this.#changed();
  }

  async save(
    adapter: Pick<LaneAdapter, "call">,
  ): Promise<RepositorySaveResult> {
    const draft = this.#draft;
    if (!draft || this.saving) {
      return { outcome: RepositorySaveOutcome.Ignored, current: !!draft };
    }
    const lifetime = this.#lifetime;
    const operation = draft.base
      ? LaneService.method.saveRepository
      : LaneService.method.createRepository;
    const displayName = draft.name || undefined;
    const key = JSON.stringify({
      operation: operation.name,
      id: draft.id,
      base: draft.base,
      url: draft.url,
      displayName,
    });
    const requestId =
      this.#attempt?.lifetime === lifetime && this.#attempt.key === key
        ? this.#attempt.requestId
        : this.#requestId();
    this.#attempt = { lifetime, key, requestId };
    this.#pending = lifetime;
    this.#changed();
    const input = {
      meta: { requestId },
      id: draft.id,
      url: draft.url,
      displayName,
    };
    const done = (
      outcome: RepositorySaveOutcome,
      error?: unknown,
    ): RepositorySaveResult => {
      if (this.#pending === lifetime) this.#pending = undefined;
      const current = this.#lifetime === lifetime;
      if (current && outcome === RepositorySaveOutcome.Saved) {
        this.#draft = undefined;
        this.#attempt = undefined;
      }
      this.#changed();
      return { outcome, current, error };
    };
    try {
      const response = draft.base
        ? await adapter.call(LaneService.method.saveRepository, {
            ...input,
            expectedVersionId: draft.base,
          })
        : await adapter.call(LaneService.method.createRepository, input);
      return done(
        response.result?.result === ResultKind.CONFLICT
          ? RepositorySaveOutcome.Conflict
          : RepositorySaveOutcome.Saved,
      );
    } catch (error) {
      return done(RepositorySaveOutcome.Failed, error);
    }
  }
}
