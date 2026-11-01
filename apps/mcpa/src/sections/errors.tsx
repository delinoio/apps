// SPDX-License-Identifier: Apache-2.0
import { useEffect } from "react";
import { useQuery } from "@connectrpc/connect-query";
import {
  EndpointFormat,
  type CallKey,
  type Resource,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc } from "../lib/hooks";
import { useCursorPager } from "../lib/pager";
import {
  callStatusInfo,
  endpointFormatLabels,
  providerLabel,
  statusClassLabels,
  Tone,
} from "../lib/labels";
import { formatBytes, formatDateTime } from "../lib/format";
import { EmptyState, QueryView } from "../components/states";
import { PagerControls } from "../components/pager";
import { CopyButton } from "../components/copy-button";
import {
  FilterBar,
  Period,
  StatusScope,
  toCallFilter,
  useFilters,
} from "./filters";

export function ErrorsView({
  tenantId,
  resourceId,
  resources,
  callKeys,
}: {
  tenantId: string;
  /** Fixes the view to one resource. */
  resourceId?: string;
  resources?: Resource[];
  callKeys?: CallKey[];
}) {
  const [filters, update] = useFilters({
    period: Period.Week,
    status: StatusScope.Failures,
  });
  const pager = useCursorPager();
  const { reset } = pager;
  useEffect(reset, [filters, reset]);
  const records = useQuery(rpc.listCallRecords, {
    tenantId,
    filter: toCallFilter(filters, resourceId),
    page: { size: 50, afterId: pager.afterId },
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
          <h2>Errors and calls</h2>
          <p className="muted">
            Call outcomes for the last 30 days. Only metadata is kept; prompts
            and outputs are never stored. Share a request ID with support to
            trace a call.
          </p>
        </div>
      </div>
      <FilterBar
        filters={filters}
        update={update}
        periods={[Period.Day, Period.Week, Period.Month]}
        resources={resourceId ? undefined : resources}
        callKeys={callKeys}
        allowFailureScope
      />
      <QueryView
        query={records}
        loadingLabel="Loading calls…"
        permissionMessage="Call records are available to resource managers. Ask a manager of the resource for access."
      >
        {(data) =>
          data.records.length ? (
            <>
              <div className="table-scroll">
                <table>
                  <caption className="sr-only">Call records</caption>
                  <thead>
                    <tr>
                      <th scope="col">Time</th>
                      {!resourceId && <th scope="col">Resource</th>}
                      <th scope="col">Outcome</th>
                      <th scope="col">Provider status</th>
                      <th scope="col">Provider / model</th>
                      <th scope="col">Call key</th>
                      <th scope="col">Format</th>
                      <th scope="col" className="num">
                        Sent to provider
                      </th>
                      <th scope="col" className="num">
                        Sent to caller
                      </th>
                      <th scope="col" className="num">
                        Latency
                      </th>
                      <th scope="col">Request ID</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.records.map((record) => {
                      const status = callStatusInfo[record.status] ?? {
                        label: "Unknown",
                        tone: Tone.Neutral,
                      };
                      return (
                        <tr key={record.id}>
                          <td>{formatDateTime(record.startTime)}</td>
                          {!resourceId && (
                            <td>{resourceName(record.resourceId)}</td>
                          )}
                          <td>
                            <span className={`status status-${status.tone}`}>
                              {status.label}
                            </span>
                            {record.errorClass && (
                              <span className="muted small block mono">
                                {record.errorClass}
                              </span>
                            )}
                          </td>
                          <td>
                            {statusClassLabels[record.providerStatusClass] ??
                              "—"}
                          </td>
                          <td>
                            {providerLabel(record.provider)}
                            <span className="muted small block mono">
                              {record.model || "—"}
                            </span>
                          </td>
                          <td>{keyName(record.callKeyId)}</td>
                          <td>
                            {endpointFormatLabels[record.format] ??
                              endpointFormatLabels[EndpointFormat.UNSPECIFIED]}
                            {record.streaming && (
                              <span className="muted small block">
                                Streaming
                              </span>
                            )}
                          </td>
                          <td
                            className="num"
                            title={`${record.requestBytes} bytes`}
                          >
                            {formatBytes(record.requestBytes)}
                          </td>
                          <td
                            className="num"
                            title={`${record.responseBytes} bytes`}
                          >
                            {formatBytes(record.responseBytes)}
                          </td>
                          <td className="num">{String(record.latencyMs)} ms</td>
                          <td>
                            <span className="mono small">
                              {record.requestId}
                            </span>{" "}
                            {record.requestId && (
                              <CopyButton
                                value={record.requestId}
                                accessibleLabel={`Copy request ID ${record.requestId}`}
                              />
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
              <PagerControls pager={pager} nextAfterId={data.nextAfterId} />
            </>
          ) : (
            <EmptyState
              title="No matching calls"
              message="Try a longer period or a different status."
            />
          )
        }
      </QueryView>
    </div>
  );
}
