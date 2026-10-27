// SPDX-License-Identifier: Apache-2.0
// Owns only renderer draft metadata. No task text or persisted state crosses
// this lifecycle boundary, and only an explicitly pending exit can be approved.
export class WorkspaceExit {
  dirty = false;
  busy = false;
  pending;
  request(action) {
    if (!this.dirty && !this.busy) return true;
    this.pending = action;
    return false;
  }
  update(raw) {
    const value = JSON.parse(raw);
    if (
      !value ||
      typeof value.dirty !== "boolean" ||
      typeof value.busy !== "boolean" ||
      typeof value.exit !== "boolean"
    )
      throw new Error("Invalid workspace state.");
    this.dirty = value.dirty;
    this.busy = value.busy;
    if (value.exit && !value.dirty && !value.busy && this.pending) {
      const action = this.pending;
      this.pending = undefined;
      return action;
    }
  }
}
