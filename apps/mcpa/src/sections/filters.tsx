// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Input } from "@packages/ui/components/input";
import { NativeSelect } from "@packages/ui/components/native-select";
import {
  CallStatus,
  Provider,
  type CallKey,
  type Resource,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import type { MessageInitShape } from "@bufbuild/protobuf";
import type { CallFilterSchema } from "@repo/protos/mcpa/v1/mcpa_pb";
import {
  callStatusInfo,
  callStatusOptions,
  providerLabels,
} from "../lib/labels";
import { Field } from "../components/field";

export enum Period {
  Day = "1",
  Week = "7",
  Month = "30",
  Quarter = "90",
  Year = "365",
}

const periodLabels: Record<Period, string> = {
  [Period.Day]: "Last 24 hours",
  [Period.Week]: "Last 7 days",
  [Period.Month]: "Last 30 days",
  [Period.Quarter]: "Last 90 days",
  [Period.Year]: "Last 12 months",
};

/** Status filter values beyond single statuses. */
export enum StatusScope {
  All = "all",
  Failures = "failures",
}

export type Filters = {
  period: Period;
  start: Date;
  resourceId: string;
  model: string;
  provider: Provider;
  callKeyId: string;
  status: CallStatus | StatusScope;
};

const periodStart = (period: Period, now = Date.now()) =>
  new Date(now - Number(period) * 86_400_000);

export function useFilters(initial: Partial<Filters> = {}) {
  const [filters, setFilters] = useState<Filters>(() => {
    const period = initial.period ?? Period.Month;
    return {
      period,
      start: periodStart(period),
      resourceId: "",
      model: "",
      provider: Provider.UNSPECIFIED,
      callKeyId: "",
      status: StatusScope.All,
      ...initial,
    };
  });
  const update = (patch: Partial<Filters>) =>
    setFilters((current) => ({
      ...current,
      ...patch,
      start: patch.period ? periodStart(patch.period) : current.start,
    }));
  return [filters, update] as const;
}

const failureStatuses = callStatusOptions.filter(
  (status) => status !== CallStatus.SUCCEEDED,
);

/** Converts console filters into the API's CallFilter message. */
export function toCallFilter(
  filters: Filters,
  fixedResourceId?: string,
): MessageInitShape<typeof CallFilterSchema> {
  const resourceId = fixedResourceId || filters.resourceId;
  const statuses =
    filters.status === StatusScope.All
      ? []
      : filters.status === StatusScope.Failures
        ? failureStatuses
        : [filters.status];
  return {
    resourceIds: resourceId ? [resourceId] : [],
    models: filters.model.trim() ? [filters.model.trim()] : [],
    providers: filters.provider ? [filters.provider] : [],
    callKeyIds: filters.callKeyId.trim() ? [filters.callKeyId.trim()] : [],
    statuses,
    startTime: timestampFromDate(filters.start),
  };
}

export function FilterBar({
  filters,
  update,
  periods,
  resources,
  callKeys,
  allowFailureScope = false,
}: {
  filters: Filters;
  update: (patch: Partial<Filters>) => void;
  periods: Period[];
  resources?: Resource[];
  callKeys?: CallKey[];
  allowFailureScope?: boolean;
}) {
  const [model, setModel] = useState(filters.model);
  const [callKeyId, setCallKeyId] = useState(filters.callKeyId);
  return (
    <form
      className="filters"
      aria-label="Filters"
      onSubmit={(event) => {
        event.preventDefault();
        update({ model, callKeyId });
      }}
    >
      <Field label="Period">
        <NativeSelect
          value={filters.period}
          onChange={(event) => update({ period: event.target.value as Period })}
        >
          {periods.map((period) => (
            <option key={period} value={period}>
              {periodLabels[period]}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {resources && (
        <Field label="Resource">
          <NativeSelect
            value={filters.resourceId}
            onChange={(event) => update({ resourceId: event.target.value })}
          >
            <option value="">All resources</option>
            {resources.map((resource) => (
              <option key={resource.id} value={resource.id}>
                {resource.name}
              </option>
            ))}
          </NativeSelect>
        </Field>
      )}
      <Field label="Provider">
        <NativeSelect
          value={String(filters.provider)}
          onChange={(event) =>
            update({ provider: Number(event.target.value) as Provider })
          }
        >
          <option value={String(Provider.UNSPECIFIED)}>All providers</option>
          {(Object.keys(providerLabels).map(Number) as Provider[])
            .filter((provider) => provider !== Provider.UNSPECIFIED)
            .map((provider) => (
              <option key={provider} value={String(provider)}>
                {providerLabels[provider]}
              </option>
            ))}
        </NativeSelect>
      </Field>
      <Field label="Status">
        <NativeSelect
          value={String(filters.status)}
          onChange={(event) => {
            const value = event.target.value;
            update({
              status:
                value === StatusScope.All || value === StatusScope.Failures
                  ? (value as StatusScope)
                  : (Number(value) as CallStatus),
            });
          }}
        >
          <option value={StatusScope.All}>All statuses</option>
          {allowFailureScope && (
            <option value={StatusScope.Failures}>All failures</option>
          )}
          {callStatusOptions.map((status) => (
            <option key={status} value={String(status)}>
              {callStatusInfo[status].label}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {callKeys ? (
        <Field label="Call key">
          <NativeSelect
            value={filters.callKeyId}
            onChange={(event) => {
              setCallKeyId(event.target.value);
              update({ callKeyId: event.target.value });
            }}
          >
            <option value="">All call keys</option>
            {callKeys.map((key) => (
              <option key={key.id} value={key.id}>
                {key.name || key.displayPrefix}
              </option>
            ))}
          </NativeSelect>
        </Field>
      ) : (
        <Field label="Call key ID">
          <Input
            value={callKeyId}
            onChange={(event) => setCallKeyId(event.target.value)}
            onBlur={() => update({ callKeyId })}
            placeholder="Any"
          />
        </Field>
      )}
      <Field label="Model">
        <Input
          value={model}
          onChange={(event) => setModel(event.target.value)}
          onBlur={() => update({ model })}
          placeholder="Any model"
        />
      </Field>
      {/* Enter in a text filter applies it. */}
      <button type="submit" hidden>
        Apply filters
      </button>
    </form>
  );
}
