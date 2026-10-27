// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
import { hasOpenDialog } from "./components";
import {
  DestinationKind,
  filterDestinations,
  nextDestination,
  type Destination,
} from "./navigation";

const icons = {
  [DestinationKind.Page]:
    '<path d="M8 6h12M8 12h12M8 18h12M3 6h1M3 12h1M3 18h1"/>',
  [DestinationKind.App]:
    '<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',
  [DestinationKind.Repository]: '<path d="M5 3h14v18H5zM9 3v18M12 7h4"/>',
};

export function CommandPalette({
  items,
  repositoryStatus,
  onSelect,
  openRequest = 0,
}: {
  openRequest?: number;
  items: Destination[];
  repositoryStatus: string;
  onSelect: (destination: Destination) => void;
}) {
  const [open, setOpen] = useState(false);
  const [newTab, setNewTab] = useState(false);
  useEffect(() => {
    if (openRequest) {
      setNewTab(true);
      setQuery("");
      setIndex(0);
      setOpen(true);
    }
  }, [openRequest]);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const results = filterDestinations(
    newTab ? items.filter((item) => item.kind !== DestinationKind.App) : items,
    query,
  );
  const selectedIndex = Math.min(index, Math.max(0, results.length - 1));
  const selected = results[selectedIndex];
  const close = () => setOpen(false);
  const choose = (destination: Destination) => {
    close();
    onSelect(destination);
  };
  const key = (value: string) => {
    if (value === "Escape") close();
    else if (value === "ArrowDown" || value === "ArrowUp")
      setIndex(
        nextDestination(
          selectedIndex,
          value === "ArrowDown" ? 1 : -1,
          results.length,
        ),
      );
    else if (value === "Enter" && selected) choose(selected);
  };
  useEffect(() => {
    "background only";
    const events = lynx.getJSModule("GlobalEventEmitter");
    const toggle = () => {
      if (!NativeModules.LaneOnline && hasOpenDialog()) return;
      setNewTab(false);
      setQuery("");
      setIndex(0);
      setOpen((previous) => !previous);
    };
    const ended = () => setOpen(false);
    events.addListener("lane.commandPalette", toggle);
    events.addListener("lane.sessionEnded", ended);
    return () => {
      events.removeListener("lane.commandPalette", toggle);
      events.removeListener("lane.sessionEnded", ended);
    };
  }, []);
  useEffect(() => {
    "background only";
    if (!open) return;
    const events = lynx.getJSModule("GlobalEventEmitter");
    const handle = (event: unknown) => {
      const value = event as { key?: unknown };
      if (typeof value?.key === "string") key(value.key);
    };
    events.addListener("lane.commandKey", handle);
    return () => events.removeListener("lane.commandKey", handle);
  }, [open, selectedIndex, selected, results.length, onSelect]);
  useEffect(() => {
    "background only";
    if (!open) return;
    lynx
      .createSelectorQuery()
      .select("#command-search")
      .invoke({ method: "focus" })
      .exec();
    return () => {
      // The browser adapter restores the exact trigger, including nested inputs.
      if (!NativeModules.LaneOnline)
        lynx
          .createSelectorQuery()
          .select("#lane-workspace")
          .invoke({ method: "setFocus", params: { focus: true } })
          .exec();
    };
  }, [open]);
  if (!open) return null;
  return (
    <view
      className="overlay command-overlay"
      global-bindkeydown={(event) => {
        if (!NativeModules.LaneOnline && event.key !== "Enter") key(event.key);
      }}
    >
      <view
        className="dialog command-palette"
        accessibility-label="Go to a page or repository"
      >
        <view className="command-search-row">
          <svg
            className="command-icon"
            content={
              '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#68758b" stroke-width="1.8"><circle cx="10" cy="10" r="7"/><path d="m15 15 6 6"/></svg>'
            }
          />
          <input
            id="command-search"
            className="input command-search"
            placeholder="Go to a page or repository..."
            accessibility-label="Go to a page or repository"
            maxlength={2147483647}
            bindconfirm={() => {
              if (!NativeModules.LaneOnline && selected) choose(selected);
            }}
            bindinput={(event) => {
              setQuery(event.detail.value);
              setIndex(0);
            }}
          />
          <view
            className="button command-close"
            accessibility-element
            accessibility-traits="button"
            accessibility-label="Close"
            focusable
            bindtap={close}
            bindkeydown={(event) => {
              if (event.key === "Enter" || event.key === " ") close();
            }}
          >
            <text>Esc</text>
          </view>
        </view>
        <scroll-view
          className="command-results"
          scroll-orientation="vertical"
          scroll-to-index={selectedIndex}
        >
          {results.map((item, itemIndex) => (
            <view key={item.id}>
              {(itemIndex === 0 ||
                results[itemIndex - 1].kind !== item.kind) && (
                <text className="command-group-label" accessibility-heading>
                  {item.kind}
                </text>
              )}
              <view
                id={item.id}
                className={`button command-result ${selected?.id === item.id ? "command-selected" : ""}`}
                accessibility-element
                accessibility-traits="button"
                accessibility-label={`${item.label}, ${item.kind}`}
                accessibility-value={selected?.id === item.id ? "Selected" : ""}
                focusable
                bindtap={() => choose(item)}
                bindkeydown={(event) => {
                  if (event.key === "Enter" || event.key === " ") choose(item);
                }}
              >
                <svg
                  className="command-icon"
                  content={`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#68758b" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${icons[item.kind]}</svg>`}
                />
                <text className="command-result-label">{item.label}</text>
                <text className="command-context">
                  {item.kind === DestinationKind.Repository
                    ? "Repository"
                    : item.context}
                </text>
              </view>
            </view>
          ))}
          {!results.length && (
            <text className="command-empty" accessibility-live-region="polite">
              No matching destinations
            </text>
          )}
          {!!repositoryStatus && (
            <text className="command-empty">{repositoryStatus}</text>
          )}
        </scroll-view>
        <view className="command-footer">
          <text className="muted">↑ ↓ Navigate</text>
          <text className="muted">↵ Open</text>
          <text className="muted">Esc Close</text>
        </view>
      </view>
    </view>
  );
}
