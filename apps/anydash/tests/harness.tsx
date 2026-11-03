// SPDX-License-Identifier: Apache-2.0
import {
  act,
  fireEvent,
  render,
  waitFor,
} from "@lynx-js/react/testing-library";
import { App } from "../src/App";
import { setHostEmitter, setHostModule } from "../src/host";
import {
  FakeEmitter,
  FakeHost,
  type FakeHostOptions,
} from "./fixtures/fake-host";

export interface Rendered {
  host: FakeHost;
  emitter: FakeEmitter;
  container: HTMLElement;
  text(): string;
  /** Waits until the rendered text contains `value`. */
  see(value: string, timeout?: number): Promise<void>;
  /** Taps the first control whose text or accessibility label is `label`. */
  tap(label: string, within?: string): Promise<void>;
  find(selector: string): HTMLElement[];
  key(element: Element, key: string, shiftKey?: boolean): Promise<void>;
  unmount(): void;
}

export async function renderApp(
  options: FakeHostOptions & { path?: string; width?: number } = {},
): Promise<Rendered> {
  const host = new FakeHost(options);
  const emitter = new FakeEmitter();
  setHostModule(host);
  setHostEmitter(emitter);
  const result = render(
    <App
      info={host.info}
      initialPath={options.path ?? "/"}
      initialWidth={options.width ?? 1280}
    />,
  );
  const container = result.container as HTMLElement;
  const text = () => container.textContent ?? "";
  const rendered: Rendered = {
    host,
    emitter,
    container,
    text,
    async see(value, timeout = 3000) {
      await waitFor(
        () => {
          if (!text().includes(value))
            throw new Error(`Missing "${value}" in: ${text().slice(0, 400)}`);
        },
        { timeout },
      );
    },
    async tap(label, within) {
      let target: Element | undefined;
      await waitFor(() => {
        const scope = within ? container.querySelector(within) : container;
        target = [
          ...(scope?.querySelectorAll(
            ".lui-button, [accessibility-role-description='link'], .ad-widget",
          ) ?? []),
        ].find(
          (element) =>
            ((
              element.querySelector(".lui-button-label")?.textContent ??
              element.textContent
            )?.trim() === label ||
              element.getAttribute("accessibility-label") === label) &&
            !element.classList.contains("lui-disabled"),
        );
        if (!target) throw new Error(`No enabled control "${label}"`);
      });
      await act(async () => {
        fireEvent.tap(target!);
      });
    },
    find(selector) {
      return [...container.querySelectorAll<HTMLElement>(selector)];
    },
    async key(element, key, shiftKey = false) {
      await act(async () => {
        fireEvent.keydown(element, { key, shiftKey });
      });
    },
    unmount() {
      result.unmount();
      setHostModule(undefined);
      setHostEmitter(undefined);
    },
  };
  return rendered;
}
