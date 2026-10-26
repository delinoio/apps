// SPDX-License-Identifier: Apache-2.0

// Owns which task-detail read may still publish editor state. Every accepted
// action that opens, replaces or closes the editor supersedes earlier reads, so a
// late response is dropped instead of replacing a newer draft, reopening a closed
// editor or pulling the user back from another page. Ownership is a counter
// rather than a task ID: reopening the same task starts a distinct lifetime.
export class EditorLifetime {
  private generation = 0;

  // Starts a new lifetime for a read and supersedes every earlier one.
  begin(): number {
    return ++this.generation;
  }

  // Invalidates pending reads without starting a new one. Call it when a
  // transition is accepted, never while a discard prompt is still undecided.
  supersede(): void {
    ++this.generation;
  }

  owns(token: number): boolean {
    return token === this.generation;
  }
}

export interface TaskReadSink<T> {
  // A missing result keeps the current view, matching the pre-existing behavior.
  opened(result: T): void;
  failed(error: unknown): void;
}

// Runs one task-detail read and publishes its outcome only while it still owns the
// editor lifetime. Success and failure are checked the same way.
export async function readTaskOwned<T>(
  lifetime: EditorLifetime,
  read: () => Promise<{ result?: T }>,
  sink: TaskReadSink<T>,
): Promise<void> {
  const token = lifetime.begin();
  try {
    const r = await read();
    if (!lifetime.owns(token)) return;
    if (r.result) sink.opened(r.result);
  } catch (e) {
    if (lifetime.owns(token)) sink.failed(e);
  }
}
