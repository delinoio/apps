// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  CaptureMode,
  desktopCaptureAvailable,
  listenRealQA,
  realqa,
  type RealQAState,
} from "./realqa-host";
export function useRealQA() {
  // Unknown until the background thread checks the host, so desktop does not
  // briefly render the mobile read-only entry.
  const [available, setAvailable] = useState<boolean>();
  const [state, setState] = useState<RealQAState>();
  const [error, setError] = useState("");
  // Mode of the in-app capture that failed most recently; offered for retry.
  const [retryMode, setRetryMode] = useState<CaptureMode>();
  const [busy, setBusy] = useState(false);
  const [discard, setDiscard] = useState(false);
  const [saved, setSaved] = useState<{ taskId: string; spaceId?: string }>();
  const [sources, setSources] = useState<{
    mode: CaptureMode;
    sources: { id: string; name: string; width: number; height: number }[];
  }>();
  const active = useRef(true);
  const count = useRef(0);
  const revision = useRef(0);
  const windowKind = useRef<RealQAState["window"]>(undefined);
  const refresh = async () => {
    const requestedRevision = revision.current;
    const result = await realqa<RealQAState>({ op: "state" });
    if (active.current && requestedRevision === revision.current) {
      windowKind.current = result.window;
      setState(result);
    }
  };
  const run = async (action: () => Promise<unknown>) => {
    count.current++;
    ++revision.current;
    setBusy(true);
    setError("");
    setRetryMode(undefined);
    try {
      await action();
      await refresh();
    } catch (e) {
      if (active.current)
        setError(
          e instanceof Error ? e.message : "Capture failed. Retry explicitly.",
        );
    } finally {
      count.current--;
      if (active.current) setBusy(count.current > 0);
    }
  };
  const updateDraft = (patch: {
    title?: string;
    description?: string;
    repositoryId?: string;
    imageId?: string;
    metadata?: import("./realqa-host").Metadata;
  }) => {
    setState((previous) => {
      if (!previous?.draft) return previous;
      const { imageId, metadata, ...fields } = patch;
      return {
        ...previous,
        draft: {
          ...previous.draft,
          ...fields,
          images: imageId
            ? previous.draft.images.map((image) =>
                image.id === imageId
                  ? { ...image, metadata: metadata || {} }
                  : image,
              )
            : previous.draft.images,
        },
      };
    });
    return run(() => realqa({ op: "updateDraft", ...patch }));
  };
  const tracked = (mode: CaptureMode, action: () => Promise<unknown>) =>
    run(async () => {
      try {
        await action();
      } catch (e) {
        // The draft is unchanged; keep the failed mode for an explicit retry.
        if (active.current) setRetryMode(mode);
        throw e;
      }
    });
  const capture = (mode: CaptureMode) =>
    tracked(mode, async () => {
      if (mode === CaptureMode.Screen || mode === CaptureMode.Window) {
        const result = await realqa<{
          sources: {
            id: string;
            name: string;
            width: number;
            height: number;
          }[];
        }>({ op: "sources", mode });
        setSources({ mode, sources: result.sources });
      } else await realqa({ op: "capture", mode });
    });
  // Capture the monitor or window chosen from the enumerated sources. A failure
  // offers the same retry as a failed enumeration.
  const captureSource = (mode: CaptureMode, sourceId: string) =>
    tracked(mode, async () => {
      setSources(undefined);
      await realqa({ op: "capture", mode, sourceId });
    });
  useEffect(() => {
    active.current = true;
    const supported = desktopCaptureAvailable();
    setAvailable(supported);
    if (!supported) return;
    const stop = listenRealQA({
      changed: () => {
        void refresh().catch((e) => setError(e.message));
      },
      composer: () => {
        void refresh().catch((e) => setError(e.message));
      },
      error: (e) => setError(e.message || "Capture failed. Retry explicitly."),
      select: (e) => {
        // Shortcut source selection belongs to the main window. The composer
        // has its own Add capture chooser and must not open a second dialog.
        if (windowKind.current !== "composer") void capture(e.mode);
      },
      closeRequested: () => setDiscard(true),
      notice: (e) => setError(e.message),
      saved: (e) => setSaved(e),
    });
    void refresh().catch((e) => setError(e.message));
    return () => {
      active.current = false;
      stop();
    };
  }, []);
  return {
    available,
    updateDraft,
    state,
    error,
    retryMode,
    setRetryMode,
    busy,
    discard,
    setDiscard,
    saved,
    setSaved,
    sources,
    setSources,
    run,
    capture,
    captureSource,
    refresh,
  };
}
export type RealQAController = ReturnType<typeof useRealQA>;
