// SPDX-License-Identifier: Apache-2.0
import { useEffect } from "react";
import { useQuery } from "@connectrpc/connect-query";
import type {
  CallKey,
  Resource,
  UsageSummary,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc } from "../lib/hooks";
import { useCursorPager } from "../lib/pager";
import { callStatusInfo, providerLabel } from "../lib/labels";
import {
  estimateUsdFromBytes,
  formatBytes,
  formatCount,
  formatDay,
  formatGib,
  formatUsd,
  PRICE_LABEL,
} from "../lib/format";
import { EmptyState, QueryView } from "../components/states";
import { PagerControls } from "../components/pager";
import { FilterBar, Period, toCallFilter, useFilters } from "./filters";

export function UsageSummaryStats({ summary }: { summary?: UsageSummary }) {
  const zero = 0n;
  return (
    <dl className="stats" aria-label="Usage summary">
      <div>
        <dt>Calls</dt>
        <dd>{formatCount(summary?.callCount ?? zero)}</dd>
      </div>
      <div>
        <dt>Sent to providers</dt>
        <dd title={`${summary?.requestBytes ?? zero} bytes`}>
          {formatBytes(summary?.requestBytes ?? zero)}
        </dd>
      </div>
      <div>
        <dt>Sent to callers</dt>
        <dd title={`${summary?.responseBytes ?? zero} bytes`}>
          {formatBytes(summary?.responseBytes ?? zero)}
        </dd>
      </div>
      <div>
        <dt>Total traffic</dt>
        <dd title={`${summary?.totalBytes ?? zero} bytes`}>
          {formatGib(summary?.totalGib)}
        </dd>
      </div>
      <div>
        <dt>Estimated cost</dt>
        <dd>{formatUsd(summary?.estimatedUsd)}</dd>
      </div>
    </dl>
  );
}

export function UsageView({
  tenantId,
  resourceId,
  resources,
  callKeys,
  title = "Usage",
  description,
}: {
  tenantId: string;
  /** Fixes the view to one resource. */
  resourceId?: string;
  resources?: Resource[];
  callKeys?: CallKey[];
  title?: string;
  description?: string;
}) {
  const [filters, update] = useFilters();
  const pager = useCursorPager();
  const { reset } = pager;
  useEffect(reset, [filters, reset]);
  const usage = useQuery(rpc.getUsage, {
    tenantId,
    filter: toCallFilter(filters, resourceId),
    page: { size: 100, afterId: pager.afterId },
  });
  const resourceName = (id: string) =>
    resources?.find((resource) => resource.id === id)?.name ?? id;
  const keyName = (id: string) => {
    const key = callKeys?.find((candidate) => candidate.id === id);
    return key ? key.name || key.displayPrefix : id ? id.slice(0, 8) : "—";
  };
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>{title}</h2>
          <p className="muted">
            {description ??
              "Measured traffic for model calls. Management in the console and MCP is free."}
          </p>
        </div>
      </div>
      <FilterBar
        filters={filters}
        update={update}
        periods={[Period.Week, Period.Month, Period.Quarter, Period.Year]}
        resources={resourceId ? undefined : resources}
        callKeys={callKeys}
      />
      <QueryView query={usage} loadingLabel="Loading usage…">
        {(data) => (
          <>
            <UsageSummaryStats summary={data.summary} />
            <p className="muted small">
              Estimated at {PRICE_LABEL} (1 GiB = 2^30 bytes) of request bytes
              sent to providers plus response bytes sent to callers. This is an
              estimate; Delcore issues the final bill.
            </p>
            {data.rows.length ? (
              <div className="table-scroll">
                <table>
                  <caption className="sr-only">Daily usage</caption>
                  <thead>
                    <tr>
                      <th scope="col">Day (UTC)</th>
                      {!resourceId && <th scope="col">Resource</th>}
                      <th scope="col">Model</th>
                      <th scope="col">Provider</th>
                      <th scope="col">Call key</th>
                      <th scope="col">Status</th>
                      <th scope="col" className="num">
                        Calls
                      </th>
                      <th scope="col" className="num">
                        Sent to providers
                      </th>
                      <th scope="col" className="num">
                        Sent to callers
                      </th>
                      <th scope="col" className="num">
                        Estimated cost
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.rows.map((row, index) => (
                      <tr
                        key={`${row.day?.seconds}-${row.resourceId}-${row.model}-${row.provider}-${row.callKeyId}-${row.status}-${index}`}
                      >
                        <td className="nowrap">{formatDay(row.day)}</td>
                        {!resourceId && <td>{resourceName(row.resourceId)}</td>}
                        <td className="mono">{row.model || "—"}</td>
                        <td>{providerLabel(row.provider)}</td>
                        <td>{keyName(row.callKeyId)}</td>
                        <td>
                          {callStatusInfo[row.status]?.label ?? "Unknown"}
                        </td>
                        <td className="num">{formatCount(row.callCount)}</td>
                        <td className="num" title={`${row.requestBytes} bytes`}>
                          {formatBytes(row.requestBytes)}
                        </td>
                        <td
                          className="num"
                          title={`${row.responseBytes} bytes`}
                        >
                          {formatBytes(row.responseBytes)}
                        </td>
                        <td className="num">
                          {estimateUsdFromBytes(
                            row.requestBytes + row.responseBytes,
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <EmptyState
                title="No usage in this period"
                message="Usage appears here after model calls are made with a call key."
              />
            )}
            <PagerControls pager={pager} nextAfterId={data.nextAfterId} />
          </>
        )}
      </QueryView>
    </div>
  );
}
