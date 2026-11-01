// SPDX-License-Identifier: Apache-2.0
import { useState } from "@lynx-js/react";
import { Button, Variant } from "@packages/lynx-ui";
import {
  UsageBreakdownDimension,
  type Connection,
  type WidgetPresentation,
  type WidgetType,
} from "@repo/protos/anydash/v1/anydash_pb";
import { providerName } from "../format";
import { ExtraCopy } from "../copy";
import {
  acceptsProvider,
  breakdownLabel,
  granularityFor,
  periodLabel,
  periods,
  presentationLabel,
  SourceKind,
  specFor,
  widgetTypes,
} from "../model/widget-types";
import { Modal } from "./dialog";
import { T } from "./primitives";

export interface NewWidget {
  type: WidgetType;
  presentation: WidgetPresentation;
  period?: (typeof periods)[number];
  breakdown?: UsageBreakdownDimension;
  connectionIds: string[];
  allDevices: boolean;
  /** Name used in the sharing confirmation. */
  name: string;
}

function Option({
  label,
  selected,
  onPress,
}: {
  label: string;
  selected: boolean;
  onPress: () => void;
}) {
  return (
    <Button
      className={`ad-option${selected ? " ad-option-selected" : ""}`}
      variant={Variant.Ghost}
      onPress={onPress}
      accessibilityLabel={`${label}${selected ? ", selected" : ""}`}
    >
      {selected ? `✓ ${label}` : label}
    </Button>
  );
}

export function AddWidgetDialog({
  connections,
  onCancel,
  onAdd,
  busy,
}: {
  connections: Connection[];
  onCancel: () => void;
  onAdd: (widget: NewWidget) => void;
  busy: boolean;
}) {
  const [type, setType] = useState<WidgetType>(widgetTypes[0].type);
  const spec = specFor(type);
  const [presentation, setPresentation] = useState(spec.presentations[0]);
  const [period, setPeriod] = useState(periods[2]);
  const [breakdown, setBreakdown] = useState(UsageBreakdownDimension.MODEL);
  const [selected, setSelected] = useState<string[]>([]);
  const eligible = connections.filter((connection) =>
    acceptsProvider(spec.source, connection.provider),
  );
  const devices = spec.source === SourceKind.Devices;
  const chosen = selected.filter((id) =>
    eligible.some((entry) => entry.id === id),
  );
  const ready = devices || chosen.length > 0;
  const name = devices
    ? spec.label
    : `${[...new Set(eligible.filter((entry) => chosen.includes(entry.id)).map((entry) => providerName(entry.provider)))].join(", ")} ${spec.label.toLowerCase()}`;
  return (
    <Modal
      title={ExtraCopy.addWidget}
      onClose={onCancel}
      action={{
        label: ExtraCopy.addWidget,
        disabled: !ready,
        loading: busy,
        onPress: () =>
          onAdd({
            type,
            presentation: spec.presentations.includes(presentation)
              ? presentation
              : spec.presentations[0],
            period: spec.period ? period : undefined,
            breakdown: spec.breakdown ? breakdown : undefined,
            connectionIds: devices ? [] : chosen,
            allDevices: devices,
            name,
          }),
      }}
    >
      <scroll-view scroll-orientation="vertical" style={{ maxHeight: "60vh" }}>
        <view className="lui-stack lui-column lui-gap-8">
          <T strong>{ExtraCopy.widgetType}</T>
          {widgetTypes.map((entry) => (
            <Option
              key={entry.type}
              label={entry.label}
              selected={entry.type === type}
              onPress={() => {
                setType(entry.type);
                setPresentation(entry.presentations[0]);
              }}
            />
          ))}
          <T strong>{ExtraCopy.presentation}</T>
          {spec.presentations.map((entry) => (
            <Option
              key={entry}
              label={presentationLabel(entry)}
              selected={entry === presentation}
              onPress={() => setPresentation(entry)}
            />
          ))}
          {spec.period ? <T strong>{ExtraCopy.period}</T> : null}
          {spec.period
            ? periods.map((entry) => (
                <Option
                  key={entry}
                  label={periodLabel(entry)}
                  selected={entry === period}
                  onPress={() => setPeriod(entry)}
                />
              ))
            : null}
          {spec.breakdown ? <T strong>{ExtraCopy.breakdown}</T> : null}
          {spec.breakdown
            ? [
                UsageBreakdownDimension.MODEL,
                UsageBreakdownDimension.TOOL,
                UsageBreakdownDimension.DEVICE,
              ].map((entry) => (
                <Option
                  key={entry}
                  label={breakdownLabel(entry)}
                  selected={entry === breakdown}
                  onPress={() => setBreakdown(entry)}
                />
              ))
            : null}
          <T strong>{ExtraCopy.source}</T>
          {devices ? (
            <T muted>{ExtraCopy.allDevices}</T>
          ) : eligible.length ? (
            eligible.map((connection) => (
              <Option
                key={connection.id}
                label={`${providerName(connection.provider)} · ${connection.displayName}`}
                selected={chosen.includes(connection.id)}
                onPress={() =>
                  setSelected((previous) =>
                    previous.includes(connection.id)
                      ? previous.filter((id) => id !== connection.id)
                      : [...previous, connection.id],
                  )
                }
              />
            ))
          ) : (
            <T muted>{ExtraCopy.noEligibleConnections}</T>
          )}
        </view>
      </scroll-view>
    </Modal>
  );
}

export const queryFor = (widget: NewWidget) => ({
  period: widget.period,
  granularity:
    widget.period !== undefined && specFor(widget.type).granularity
      ? granularityFor(widget.period)
      : undefined,
  breakdown: widget.breakdown,
});
