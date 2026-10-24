// SPDX-License-Identifier: Apache-2.0
import { cleanup } from "@testing-library/react";
import { afterEach } from "@rstest/core";

afterEach(cleanup);

// jsdom has no layout or ResizeObserver. Radix uses it to size hidden form
// controls. These tests check semantics only; remove when jsdom supports it.
globalThis.ResizeObserver = class implements ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
};
