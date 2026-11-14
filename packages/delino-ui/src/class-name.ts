// SPDX-License-Identifier: Apache-2.0

/** A UI SDK part `className`: a string, or a function of the part's state. */
export type PartClassName<State> =
  string | ((state: State) => string | undefined) | undefined;

/**
 * Prepends the package's own classes to a caller's `className`. It keeps the
 * state-function form that UI SDK parts accept.
 */
export function mergeClassName<State>(
  base: string,
  className: PartClassName<State>,
): string | ((state: State) => string) {
  if (typeof className === "function") {
    return (state) => joinClassNames(base, className(state));
  }
  return joinClassNames(base, className);
}

function joinClassNames(base: string, extra: string | undefined): string {
  return extra ? `${base} ${extra}` : base;
}
