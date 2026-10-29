// SPDX-License-Identifier: Apache-2.0

// Owns which task-detail read may still publish editor state. Every accepted
// action that opens, replaces or closes the editor supersedes earlier reads, so a
// late response is dropped instead of replacing a newer draft, reopening a closed
// editor or pulling the user back from another page. Ownership is a counter
// rather than a task ID: reopening the same task starts a distinct lifetime.
//
// Mutations submitted from an editor follow the same rule through a separate
// editor counter: it advances only when the editor shown to the user is replaced
// or closed, so a read that fails or returns nothing never strands a save that was
// submitted by the editor that is still on screen.
export class EditorLifetime {
  private generation = 0;
  private editor = 0;

  // Starts a new lifetime for a read and supersedes every earlier one.
  begin(): number {
    return ++this.generation;
  }

  // Invalidates pending reads and mutation completions without starting a new
  // read. Call it when a transition is accepted, never while a discard prompt is
  // still undecided.
  supersede(): void {
    ++this.generation;
    ++this.editor;
  }

  // Invalidates pending reads only. Use it for an accepted transition that keeps
  // the editor on screen until a new read publishes a task: a mutation submitted
  // by that editor keeps its right to apply its completion if the read fails or
  // returns nothing. A successful read replaces the editor through
  // `readTaskOwned`; any other code path that clears the editor must call
  // `replaceEditor` or `supersede` itself.
  supersedeReads(): void {
    ++this.generation;
  }

  owns(token: number): boolean {
    return token === this.generation;
  }

  // Marks the editor on screen as replaced by another one (for example a read
  // that publishes a task). Completions started by the previous editor are dropped.
  replaceEditor(): void {
    ++this.editor;
  }

  // Captures the editor that is on screen now, for a mutation about to start.
  editorToken(): number {
    return this.editor;
  }

  ownsEditor(token: number): boolean {
    return token === this.editor;
  }
}

export interface EditorCompletionSink<T> {
  // Applies a result to the editor and refreshes dependent read data.
  saved(result?: T): void;
  // Refreshes dependent read data only. It never touches the editor state.
  refresh(): void;
  // Closes the editor after a completed deletion.
  close(): void;
}

export interface EditorCompletion<T> {
  saved(result?: T): void;
  close(): void;
}

// Binds the completion of one editor mutation to the editor that submitted it.
// While that editor is still on screen the result is applied normally. Once it was
// closed, discarded or replaced (even by a new editor for the same task), the
// submitted server mutation is not rolled back; it only refreshes read data and
// must not replace, clean, reopen or close another editor.
export function beginEditorCompletion<T>(
  lifetime: EditorLifetime,
  sink: EditorCompletionSink<T>,
): EditorCompletion<T> {
  const token = lifetime.editorToken();
  return {
    saved: (result) => {
      if (lifetime.ownsEditor(token)) sink.saved(result);
      else sink.refresh();
    },
    close: () => {
      if (lifetime.ownsEditor(token)) sink.close();
    },
  };
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
    if (r.result) {
      // Publishing replaces the editor on screen, so earlier editors lose their
      // right to apply mutation completions.
      lifetime.replaceEditor();
      sink.opened(r.result);
    }
  } catch (e) {
    if (lifetime.owns(token)) sink.failed(e);
  }
}
