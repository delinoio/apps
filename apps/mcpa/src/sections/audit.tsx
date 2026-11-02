// SPDX-License-Identifier: Apache-2.0
import { useQuery } from "@connectrpc/connect-query";
import { useSession } from "../app";
import { rpc } from "../lib/hooks";
import { useCursorPager } from "../lib/pager";
import { actorKindLabels, auditActionLabels } from "../lib/labels";
import { formatDateTime } from "../lib/format";
import { EmptyState, QueryView } from "../components/states";
import { PagerControls } from "../components/pager";

export function AuditView({ resourceId }: { resourceId: string }) {
  const { session } = useSession();
  const pager = useCursorPager();
  const events = useQuery(rpc.listAuditEvents, {
    resourceId,
    page: { size: 50, afterId: pager.afterId },
  });
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Audit log</h2>
          <p className="muted">
            Changes to this resource's credentials, call keys, access, routing
            and budget. Kept for 13 months.
          </p>
        </div>
      </div>
      <QueryView
        query={events}
        loadingLabel="Loading audit log…"
        permissionMessage="Only resource managers can view the audit log."
      >
        {(data) =>
          data.events.length ? (
            <>
              <ol className="timeline">
                {data.events.map((event) => {
                  const details = Object.entries(event.details);
                  return (
                    <li key={event.id}>
                      <div className="timeline-head">
                        <strong>
                          {auditActionLabels[event.action] ?? "Change"}
                        </strong>
                        <span className="muted small">
                          {formatDateTime(event.occurTime)}
                        </span>
                      </div>
                      <p className="small">
                        By{" "}
                        {event.actorUserId === session.userId
                          ? "you"
                          : event.actorUserId || "the system"}{" "}
                        through {actorKindLabels[event.actorKind] ?? "unknown"}
                        {event.targetId && (
                          <>
                            {" · "}target{" "}
                            <span className="mono">{event.targetId}</span>
                          </>
                        )}
                      </p>
                      {details.length > 0 && (
                        <dl className="details">
                          {details.map(([key, value]) => (
                            <div key={key}>
                              <dt>{key}</dt>
                              <dd>{value}</dd>
                            </div>
                          ))}
                        </dl>
                      )}
                    </li>
                  );
                })}
              </ol>
              <PagerControls pager={pager} nextAfterId={data.nextAfterId} />
            </>
          ) : (
            <EmptyState
              title="No changes yet"
              message="Changes to this resource appear here."
            />
          )
        }
      </QueryView>
    </div>
  );
}
