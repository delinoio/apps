// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  CaptureService,
  LaneService,
  ResultKind,
  type EntityResult,
} from "@repo/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import { newId, type LaneAdapter } from "./adapter";
import { Button, Choices, Dialog, Field, Notice } from "./components";
import { RealQADialogs } from "./RealQA";
import {
  EditTool,
  captureModes,
  realqa,
  type CaptureDraft,
  type DraftImage,
  type Layer,
  type Point,
} from "./realqa-host";
import type { RealQAController } from "./useRealQA";
import { conflicted, repositoryName } from "./model";
import { connectionOf } from "./URLConnections";
import {
  captureValidation,
  decodePNG,
  matchingRepository,
} from "./captures-model";

export function CaptureComposer({
  controller,
  adapter,
}: {
  controller: RealQAController;
  adapter?: LaneAdapter;
}) {
  const draft = controller.state?.draft;
  const [repositories, setRepositories] = useState<UI<EntityResult>[]>([]);
  const [repositoriesReady, setRepositoriesReady] = useState(false);
  const [repositoryError, setRepositoryError] = useState("");
  const [rulesError, setRulesError] = useState("");
  const [lookupRetry, setLookupRetry] = useState(0);
  const [captureMenu, setCaptureMenu] = useState(false);
  const [returnToAddCapture, setReturnToAddCapture] = useState(false);
  const [selected, setSelected] = useState("");
  const [narrow, setNarrow] = useState(false);
  const [rules, setRules] =
    useState<{ prefix: string; repositoryId: string }[]>();
  const [rulesReady, setRulesReady] = useState(false);
  const matched = useRef<string | undefined>(undefined);
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);
  const saveLock = useRef(false);
  const identity = useRef<
    | { draftId?: string; taskId: string; requestId: string; key?: string }
    | undefined
  >(undefined);
  const image =
    draft?.images.find((i) => i.id === selected) || draft?.images[0];
  const options = repositories
    .filter((r) => r.version && !r.version.deleted && !conflicted(r))
    .map((r) => [r.version!.entityId, repositoryName(r.version)] as const);
  const validation = captureValidation(
    draft,
    options.map(([id]) => id),
  );
  const invalid = Object.values(validation).some(Boolean);
  useEffect(() => {
    if (!adapter) return;
    let active = true;
    let repositoryRequest = 0;
    let ruleRequest = 0;
    const loadRepositories = async () => {
      // Composer windows replace the tab tree, so their repository choices must
      // be loaded by the composer rather than by a retained Lane workspace.
      const request = ++repositoryRequest;
      setRepositoriesReady(false);
      try {
        const repositoryResults: UI<EntityResult>[] = [];
        let repositoryCursor = "";
        do {
          const r = await adapter.call(LaneService.method.listRepositories, {
            page: { size: 100, cursor: repositoryCursor },
          });
          if (!active || request !== repositoryRequest) return;
          repositoryResults.push(...r.items);
          repositoryCursor = r.nextCursor;
        } while (repositoryCursor);
        if (!active || request !== repositoryRequest) return;
        setRepositories(repositoryResults);
        setRepositoryError("");
        setRepositoriesReady(true);
      } catch (e) {
        if (active && request === repositoryRequest) {
          setRepositoryError(
            e instanceof Error ? e.message : "Repository lookup failed.",
          );
          setRepositoriesReady(false);
        }
      }
    };
    const loadRules = async () => {
      const request = ++ruleRequest;
      setRulesReady(false);
      try {
        const all: { prefix: string; repositoryId: string }[] = [];
        let cursor = "";
        do {
          const r = await adapter.call(
            CaptureService.method.listURLConnections,
            {
              page: { size: 100, cursor },
            },
          );
          if (!active || request !== ruleRequest) return;
          for (const item of r.items) {
            const value = connectionOf(item.version);
            if (value && !conflicted(item) && !item.version?.deleted)
              all.push(value);
          }
          cursor = r.nextCursor;
        } while (cursor);
        if (!active || request !== ruleRequest) return;
        setRules(all);
        setRulesError("");
        setRulesReady(true);
      } catch (e) {
        if (active && request === ruleRequest) {
          setRulesError(
            e instanceof Error ? e.message : "URL connection lookup failed.",
          );
          setRulesReady(false);
        }
      }
    };
    const load = () => {
      // Either lookup can fail independently; a slow repository list must not
      // suppress URL-rule selection or warnings for the current capture.
      void loadRepositories();
      void loadRules();
    };
    load();
    const stop = adapter.subscribe(load);
    return () => {
      active = false;
      stop();
    };
  }, [adapter, lookupRetry]);
  useEffect(() => {
    if (!draft || !rules || !draft.images.length) return;
    const key = draft.id || draft.images[0].id;
    if (matched.current === key) return;
    matched.current = key;
    const repositoryId = matchingRepository(
      draft.images[0].metadata.pageUrl,
      rules,
    );
    if (repositoryId && !draft.repositoryId)
      void controller.run(() => realqa({ op: "updateDraft", repositoryId }));
  }, [draft?.id, draft?.images.length, rules]);
  const update = (patch: Parameters<typeof controller.updateDraft>[0]) => {
    void controller.updateDraft(patch);
  };
  const save = async () => {
    if (saveLock.current || !adapter || !draft || invalid || !repositoriesReady)
      return;
    const spaceId = adapter.spaceId;
    saveLock.current = true;
    setSaving(true);
    setSaveError("");
    try {
      if (!spaceId)
        throw new Error(
          "The data space is unavailable. Reopen Lane before saving.",
        );
      const exported = await realqa<CaptureDraft>({ op: "export" });
      if (adapter.spaceId !== spaceId)
        throw new Error(
          "The data space changed. Your draft is retained; reopen it in the original space before saving.",
        );
      const key = JSON.stringify(exported);
      if (!identity.current || identity.current.draftId !== exported.id)
        identity.current = {
          draftId: exported.id,
          taskId: exported.id || newId(),
          requestId: exported.requestId || newId(),
        };
      if (identity.current.key && identity.current.key !== key)
        identity.current.requestId = newId();
      identity.current.key = key;
      const images = exported.images.map((item) => {
        if (!item.sha256 || !/^[a-f0-9]{64}$/.test(item.sha256))
          throw new Error(
            "The capture host did not provide a final image checksum. Update Lane and retry.",
          );
        const png = decodePNG(item.png);
        return {
          png,
          capture: {
            attachmentId: item.id,
            width: item.width,
            height: item.height,
            sha256: item.sha256,
            byteSize: String(png.length),
            ...item.metadata,
          },
        };
      });
      const response = await adapter.call(
        CaptureService.method.createCaptureTask,
        {
          meta: { requestId: identity.current.requestId },
          id: identity.current.taskId,
          repositoryId: exported.repositoryId,
          title: exported.title,
          description: exported.description,
          images,
        },
      );
      if (
        !response.result?.version ||
        response.result.result === ResultKind.CONFLICT ||
        response.result.result === ResultKind.DELETED
      )
        throw new Error(
          "Lane did not confirm a saved task. Your draft is retained; retry explicitly.",
        );
      await realqa({
        op: "complete",
        taskId: identity.current.taskId,
        spaceId,
      });
    } catch (e) {
      await realqa({ op: "saveFailed" }).catch(() => {});
      setSaveError(
        e instanceof Error
          ? e.message
          : "Saving failed. Your draft is retained.",
      );
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };
  const warning =
    rules &&
    draft?.images.some((item) => {
      const matchedRepository = matchingRepository(
        item.metadata.pageUrl,
        rules,
      );
      return matchedRepository && matchedRepository !== draft.repositoryId;
    });
  return (
    <view
      className={`realqa-composer ${narrow ? "compact" : ""}`}
      bindlayoutchange={(e) => setNarrow(e.detail.width < 900)}
      global-bindkeydown={(e) => {
        if (
          e.key === "Escape" &&
          !controller.sources &&
          !controller.discard &&
          !captureMenu &&
          !saving
        )
          controller.setDiscard(true);
      }}
    >
      <view className="row between composer-header">
        <view className="composer-brand">
          <text className="heading">RealQA</text>
          <text className="muted">Capture a problem</text>
        </view>
        <Button disabled={saving} onClick={() => controller.setDiscard(true)}>
          Close
        </Button>
      </view>
      <scroll-view className="composer-scroll" scroll-orientation="vertical">
        <view className="composer-columns">
          <view className="composer-editor">
            {!draft ? (
              <Notice>Loading captures…</Notice>
            ) : image ? (
              <ImageEditor
                key={image.id}
                image={image}
                controller={controller}
                disabled={saving}
              />
            ) : (
              <Notice>No captures yet. Add a capture to start.</Notice>
            )}
            <view className="composer-captures">
              <text className="heading">Captures</text>
              {image && (
                <text className="muted composer-identity">
                  Selected capture ID: {image.id}
                </text>
              )}
              <view className="row wrap thumbnails">
                {draft?.images.map((item, index) => (
                  <view className="capture-thumbnail" key={item.id}>
                    <image
                      src={`data:image/png;base64,${item.png}`}
                      mode="aspectFit"
                      className="thumbnail-image"
                      accessibility-label={`Capture ${index + 1}`}
                    />
                    <Button
                      pressed={item.id === image?.id}
                      label={`Capture ${index + 1} of ${draft.images.length}${item.id === image?.id ? ", selected" : ""}`}
                      disabled={saving}
                      onClick={() => setSelected(item.id)}
                    >{`Capture ${index + 1} of ${draft.images.length}`}</Button>
                    <view className="row wrap">
                      <Button
                        label={`Move capture ${index + 1} earlier`}
                        disabled={saving || controller.busy || index === 0}
                        onClick={() =>
                          void controller.run(() =>
                            realqa({
                              op: "reorderImages",
                              ids: move(
                                draft.images.map((i) => i.id),
                                index,
                                -1,
                              ),
                            }),
                          )
                        }
                      >
                        Move earlier
                      </Button>
                      <Button
                        label={`Move capture ${index + 1} later`}
                        disabled={
                          saving ||
                          controller.busy ||
                          index === draft.images.length - 1
                        }
                        onClick={() =>
                          void controller.run(() =>
                            realqa({
                              op: "reorderImages",
                              ids: move(
                                draft.images.map((i) => i.id),
                                index,
                                1,
                              ),
                            }),
                          )
                        }
                      >
                        Move later
                      </Button>
                      <Button
                        label={`Remove capture ${index + 1}`}
                        disabled={saving || controller.busy}
                        onClick={() =>
                          void controller.run(() =>
                            realqa({ op: "removeImage", imageId: item.id }),
                          )
                        }
                      >
                        Remove capture
                      </Button>
                    </view>
                  </view>
                ))}
              </view>
            </view>
            {validation.images && (
              <text className="validation">{validation.images}</text>
            )}
            <Button
              disabled={saving || controller.busy}
              expanded={captureMenu}
              focusOnShow={returnToAddCapture && !captureMenu}
              onClick={() => {
                setReturnToAddCapture(false);
                setCaptureMenu(true);
              }}
            >
              Add capture
            </Button>
          </view>
          <view className="composer-form">
            <text className="heading">Task details</text>
            {!adapter && (
              <Notice>
                Lane storage is unavailable. Open the main Lane window to
                resolve the storage error before saving.
              </Notice>
            )}
            {adapter && !repositoriesReady && !repositoryError && (
              <Notice>Loading repositories…</Notice>
            )}
            {repositoryError && (
              <Notice>
                Repository choices could not load: {repositoryError} Your draft
                is retained.
              </Notice>
            )}
            {repositoryError && (
              <Button
                disabled={saving}
                onClick={() => setLookupRetry((value) => value + 1)}
              >
                Retry repository lookup
              </Button>
            )}
            {adapter && !rulesReady && !rulesError && (
              <Notice>Loading URL connections…</Notice>
            )}
            {rulesError && (
              <Notice>
                URL connections could not load: {rulesError} Select a repository
                manually or retry.
              </Notice>
            )}
            {rulesError && (
              <Button
                disabled={saving}
                onClick={() => setLookupRetry((value) => value + 1)}
              >
                Retry URL connection lookup
              </Button>
            )}
            <Choices
              label="Repository"
              value={draft?.repositoryId || ""}
              options={options}
              disabled={saving || !repositoriesReady}
              onChange={(repositoryId) => update({ repositoryId })}
            />
            {validation.repository && (
              <text className="validation">{validation.repository}</text>
            )}
            <Field
              label="Title"
              focus
              value={draft?.title || ""}
              disabled={saving}
              onChange={(title) => update({ title })}
            />
            {validation.title && (
              <text className="validation">{validation.title}</text>
            )}
            <Field
              label="Description"
              multiline
              value={draft?.description || ""}
              disabled={saving}
              onChange={(description) => update({ description })}
            />
            {image && (
              <view className="section">
                <text className="heading">Capture details</text>
                <text className="muted">
                  Capture{" "}
                  {draft?.images.findIndex((item) => item.id === image.id)! + 1}{" "}
                  of {draft?.images.length}
                </text>
                {warning && (
                  <Notice>
                    A capture matches a different repository. Your selected
                    repository has not changed.
                  </Notice>
                )}
                {(
                  [
                    ["pageUrl", "URL"],
                    ["pageTitle", "Page title"],
                    ["cssSelector", "CSS selector"],
                  ] as const
                ).map(([key, label]) => (
                  <view key={key}>
                    <Field
                      label={label}
                      value={image.metadata[key] || ""}
                      disabled={saving}
                      onChange={(value) =>
                        update({
                          imageId: image.id,
                          metadata: { ...image.metadata, [key]: value },
                        })
                      }
                    />
                    {image.metadata[key] !== undefined && (
                      <Button
                        disabled={saving}
                        onClick={() => {
                          const metadata = { ...image.metadata };
                          delete metadata[key];
                          update({ imageId: image.id, metadata });
                        }}
                      >{`Remove ${label}`}</Button>
                    )}
                  </view>
                ))}
              </view>
            )}
            {controller.error && <Notice>{controller.error}</Notice>}
            {saveError && <Notice>{saveError} Your draft is retained.</Notice>}
            <text className="muted">
              Unsaved captures and layers stay in memory until you save or
              discard this draft.
            </text>
            {!!draft?.images.length && (
              <Button
                primary
                disabled={
                  saving ||
                  controller.busy ||
                  !adapter ||
                  !repositoriesReady ||
                  invalid
                }
                onClick={() => void save()}
              >
                {saving ? "Saving…" : saveError ? "Retry" : "Save to Lane"}
              </Button>
            )}
          </view>
        </view>
      </scroll-view>
      {captureMenu && (
        <Dialog
          title="Add capture"
          onClose={() => {
            setCaptureMenu(false);
            setReturnToAddCapture(true);
          }}
        >
          {captureModes.map(([mode, label]) => (
            <Button
              key={mode}
              disabled={saving || controller.busy}
              onClick={() => {
                setCaptureMenu(false);
                setReturnToAddCapture(true);
                void controller.capture(mode);
              }}
            >
              {label}
            </Button>
          ))}
        </Dialog>
      )}
      <RealQADialogs controller={controller} />
    </view>
  );
}
export function move<T>(values: T[], index: number, direction: number): T[] {
  const next = [...values];
  const target = index + direction;
  if (target < 0 || target >= next.length) return next;
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}
function ImageEditor({
  image,
  controller,
  disabled,
}: {
  image: DraftImage;
  controller: RealQAController;
  disabled: boolean;
}) {
  const [tool, setTool] = useState(EditTool.Arrow);
  const [coordinatesExpanded, setCoordinatesExpanded] = useState(false);
  const [text, setText] = useState("");
  const [color, setColor] = useState("#075AFF");
  const [box, setBox] = useState({
    x: "0",
    y: "0",
    width: "100",
    height: "100",
  });
  const [points, setPoints] = useState("0,0 100,100");
  const [error, setError] = useState("");
  const [size, setSize] = useState({ width: 1, height: 1 });
  const gesture = useRef<Point[]>([]);
  const apply = (path?: Point[]) => {
    if (disabled || controller.busy) return;
    setError("");
    try {
      if (Object.values(box).some((value) => !value.trim()))
        throw new Error(
          "Enter valid pixel coordinates and positive dimensions.",
        );
      const x = Number(box.x),
        y = Number(box.y),
        width = Number(box.width),
        height = Number(box.height);
      const needsPoints = tool === EditTool.Draw || tool === EditTool.Arrow;
      const line =
        path ||
        (needsPoints
          ? points
              .trim()
              .split(/\s+/)
              .map((pair) => {
                if (!/^-?\d+(?:\.\d+)?,-?\d+(?:\.\d+)?$/.test(pair))
                  throw new Error(
                    "Enter points as x,y pairs separated by spaces.",
                  );
                const [pointX, pointY] = pair.split(",").map(Number);
                return { x: pointX, y: pointY };
              })
          : []);
      if (needsPoints && line.length < 2)
        throw new Error("Enter at least two x,y points.");
      if (
        ![x, y, width, height].every(Number.isFinite) ||
        width <= 0 ||
        height <= 0 ||
        line.some((p) => !Number.isFinite(p.x) || !Number.isFinite(p.y))
      )
        throw new Error(
          "Enter valid pixel coordinates and positive dimensions.",
        );
      if (!/^#[0-9a-f]{6}$/i.test(color))
        throw new Error("Enter a six-digit hex color, such as #075AFF.");
      const start = path?.[0];
      const end = path?.[path.length - 1];
      const rect =
        start && end
          ? {
              x: Math.min(start.x, end.x),
              y: Math.min(start.y, end.y),
              width: Math.max(1, Math.abs(end.x - start.x)),
              height: Math.max(1, Math.abs(end.y - start.y)),
            }
          : { x, y, width, height };
      if (tool === EditTool.Crop) {
        void controller.run(() =>
          realqa({
            op: "edit",
            imageId: image.id,
            layers: image.layers,
            crop: rect,
          }),
        );
        return;
      }
      if (tool === EditTool.Text && !text.trim())
        throw new Error("Enter annotation text.");
      const layer: Layer = {
        type: tool,
        ...rect,
        color,
        strokeWidth: 3,
        fontSize: 24,
      };
      if (tool === EditTool.Draw || tool === EditTool.Arrow)
        layer.points = line;
      if (tool === EditTool.Text) layer.text = text;
      void controller.run(() =>
        realqa({
          op: "edit",
          imageId: image.id,
          layers: [...image.layers, layer],
          crop: image.crop,
        }),
      );
    } catch (e) {
      setError((e as Error).message);
    }
  };
  const point = (value: { x: number; y: number }): Point => {
    const scale = Math.min(
      size.width / image.width,
      size.height / image.height,
    );
    const left = (size.width - image.width * scale) / 2,
      top = (size.height - image.height * scale) / 2;
    return {
      x:
        Math.round(
          Math.max(0, Math.min(image.width, (value.x - left) / scale)),
        ) + (image.crop?.x || 0),
      y:
        Math.round(
          Math.max(0, Math.min(image.height, (value.y - top) / scale)),
        ) + (image.crop?.y || 0),
    };
  };
  return (
    <view className="section image-editor">
      <view className="row wrap editor-tools">
        {Object.values(EditTool).map((value) => (
          <Button
            key={value}
            primary={tool === value}
            disabled={disabled}
            onClick={() => {
              setTool(value);
              if (value === EditTool.Text) setCoordinatesExpanded(true);
            }}
          >
            {
              {
                crop: "Crop",
                draw: "Freehand",
                arrow: "Arrow",
                rectangle: "Rectangle",
                text: "Text",
                blur: "Blur",
                cover: "Opaque cover",
              }[value]
            }
          </Button>
        ))}
        <Button
          disabled={disabled || controller.busy || image.canUndo === false}
          onClick={() =>
            void controller.run(() => realqa({ op: "undo", imageId: image.id }))
          }
        >
          Undo
        </Button>
        <Button
          disabled={disabled || controller.busy || image.canRedo === false}
          onClick={() =>
            void controller.run(() => realqa({ op: "redo", imageId: image.id }))
          }
        >
          Redo
        </Button>
      </view>
      <view
        className="image-edit-surface"
        accessibility-label="Capture editor. Drag to annotate, or use the pixel coordinate fields below."
        bindlayoutchange={(e) =>
          setSize({ width: e.detail.width, height: e.detail.height })
        }
        bindtouchstart={(e) => {
          if (!disabled && e.touches[0])
            gesture.current = [point(e.touches[0])];
        }}
        bindtouchmove={(e) => {
          if (gesture.current.length && e.touches[0])
            gesture.current.push(point(e.touches[0]));
        }}
        bindtouchcancel={() => {
          gesture.current = [];
        }}
        bindtouchend={(e) => {
          if (!gesture.current.length) return;
          if (e.changedTouches[0])
            gesture.current.push(point(e.changedTouches[0]));
          const path = gesture.current;
          gesture.current = [];
          apply(path);
        }}
      >
        <image
          className="editor-image"
          src={`data:image/png;base64,${image.png}`}
          mode="aspectFit"
          accessibility-label="Current edited capture"
        />
      </view>
      <text className="muted">
        {image.width} × {image.height} pixels. Drag on the image, or enter pixel
        coordinates below and apply. Layers remain in memory until saved or
        discarded.
      </text>
      <Button
        expanded={coordinatesExpanded}
        disabled={disabled}
        onClick={() => setCoordinatesExpanded((value) => !value)}
      >
        Pixel coordinates
      </Button>
      {coordinatesExpanded && (
        <view className="composer-coordinates">
          <view className="composer-coordinate-grid">
            {(["x", "y", "width", "height"] as const).map((key) => (
              <Field
                key={key}
                label={key}
                value={box[key]}
                disabled={disabled}
                onChange={(value) => setBox({ ...box, [key]: value })}
              />
            ))}
          </view>
          {(tool === EditTool.Draw || tool === EditTool.Arrow) && (
            <Field
              label="Points (x,y pairs separated by spaces)"
              value={points}
              disabled={disabled}
              onChange={setPoints}
            />
          )}
          {tool === EditTool.Text && (
            <Field
              label="Annotation text"
              focus
              value={text}
              disabled={disabled}
              onChange={setText}
            />
          )}
          <Field
            label="Annotation color"
            value={color}
            disabled={disabled}
            onChange={setColor}
          />
          <Button
            disabled={disabled || controller.busy}
            onClick={() => apply()}
          >
            Apply tool
          </Button>
        </view>
      )}
      {error && <Notice>{error}</Notice>}
      <text className="label">Annotation layers (back to front)</text>
      {!image.layers.length && <text>No annotation layers</text>}
      {image.layers.map((layer, index) => (
        <view className="row wrap composer-layer" key={index}>
          <text>
            {index + 1}. {layer.type}
            {layer.text ? `: ${layer.text}` : ""}
          </text>
          <Button
            label={`Move layer ${index + 1} up`}
            disabled={
              disabled || controller.busy || index === image.layers.length - 1
            }
            onClick={() =>
              void controller.run(() =>
                realqa({
                  op: "edit",
                  imageId: image.id,
                  layers: move(image.layers, index, 1),
                  crop: image.crop,
                }),
              )
            }
          >
            Move up
          </Button>
          <Button
            label={`Move layer ${index + 1} down`}
            disabled={disabled || controller.busy || index === 0}
            onClick={() =>
              void controller.run(() =>
                realqa({
                  op: "edit",
                  imageId: image.id,
                  layers: move(image.layers, index, -1),
                  crop: image.crop,
                }),
              )
            }
          >
            Move down
          </Button>
          <Button
            label={`Remove layer ${index + 1}`}
            disabled={disabled || controller.busy}
            onClick={() =>
              void controller.run(() =>
                realqa({
                  op: "edit",
                  imageId: image.id,
                  layers: image.layers.filter((_, i) => i !== index),
                  crop: image.crop,
                }),
              )
            }
          >
            Remove layer
          </Button>
        </view>
      ))}
    </view>
  );
}
