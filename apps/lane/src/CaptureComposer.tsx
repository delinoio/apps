// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "@lynx-js/react";
import {
  CaptureService,
  ResultKind,
  type EntityResult,
} from "@delino/protos/lane/v1/lane_pb";
import type { UI } from "./serializable";
import { newId, type LaneAdapter } from "./adapter";
import { Button, Choices, Field, Notice } from "./components";
import { CaptureActions, RealQADialogs } from "./RealQA";
import {
  EditTool,
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
  repositories,
}: {
  controller: RealQAController;
  adapter?: LaneAdapter;
  repositories: UI<EntityResult>[];
}) {
  const draft = controller.state?.draft;
  const [selected, setSelected] = useState("");
  const [narrow, setNarrow] = useState(false);
  const [rules, setRules] =
    useState<{ prefix: string; repositoryId: string }[]>();
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
    const load = async () => {
      const all: { prefix: string; repositoryId: string }[] = [];
      let cursor = "";
      do {
        const r = await adapter.call(CaptureService.method.listURLConnections, {
          page: { size: 100, cursor },
        });
        for (const item of r.items) {
          const value = connectionOf(item.version);
          if (value && !conflicted(item) && !item.version?.deleted)
            all.push(value);
        }
        cursor = r.nextCursor;
      } while (cursor);
      if (active) setRules(all);
    };
    void load().catch((e) => setSaveError(e.message));
    const stop = adapter.subscribe(() => {
      void load().catch((e) => setSaveError(e.message));
    });
    return () => {
      active = false;
      stop();
    };
  }, [adapter]);
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
    if (saveLock.current || !adapter || !draft || invalid) return;
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
          !saving
        )
          controller.setDiscard(true);
      }}
    >
      <view className="row between composer-header">
        <text className="heading">RealQA</text>
        <Button disabled={saving} onClick={() => controller.setDiscard(true)}>
          Close
        </Button>
      </view>
      <scroll-view className="composer-scroll" scroll-orientation="vertical">
        <view className="composer-columns">
          <view className="composer-editor">
            {image ? (
              <ImageEditor
                key={image.id}
                image={image}
                controller={controller}
                disabled={saving}
              />
            ) : (
              <Notice>No captures yet. Add a capture to start.</Notice>
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
                    primary={item.id === image?.id}
                    disabled={saving}
                    onClick={() => setSelected(item.id)}
                  >{`Capture ${index + 1} of ${draft.images.length}`}</Button>
                  <view className="row wrap">
                    <Button
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
            <text className="label">Add capture</text>
            {validation.images && (
              <text className="validation">{validation.images}</text>
            )}
            {!saving && <CaptureActions controller={controller} />}
          </view>
          <view className="composer-form">
            {!adapter && (
              <Notice>
                Lane storage is unavailable. Open the main Lane window to
                resolve the storage error before saving.
              </Notice>
            )}
            <Choices
              label="Repository"
              value={draft?.repositoryId || ""}
              options={options}
              disabled={saving}
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
            <Button
              primary
              disabled={saving || controller.busy || !adapter || invalid}
              onClick={() => void save()}
            >
              {saving ? "Saving…" : saveError ? "Retry" : "Save to Lane"}
            </Button>
          </view>
        </view>
      </scroll-view>
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
      const x = Number(box.x),
        y = Number(box.y),
        width = Number(box.width),
        height = Number(box.height);
      const line =
        path ||
        points
          .trim()
          .split(/\s+/)
          .map((pair) => {
            const [x, y] = pair.split(",").map(Number);
            return { x, y };
          });
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
            onClick={() => setTool(value)}
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
      <view className="row wrap">
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
      <Button disabled={disabled || controller.busy} onClick={() => apply()}>
        Apply tool
      </Button>
      {error && <Notice>{error}</Notice>}
      <text className="label">Annotation layers (back to front)</text>
      {!image.layers.length && <text>No annotation layers</text>}
      {image.layers.map((layer, index) => (
        <view className="row wrap" key={index}>
          <text>
            {index + 1}. {layer.type}
            {layer.text ? `: ${layer.text}` : ""}
          </text>
          <Button
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
