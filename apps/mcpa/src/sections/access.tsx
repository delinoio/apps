// SPDX-License-Identifier: Apache-2.0
import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { NativeSelect } from "@packages/ui/components/native-select";
import {
  ErrorReason,
  PrincipalKind,
  ResourceRole,
  type AclEntry,
  type Group,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { useSession } from "../app";
import { rpc, useInvalidate } from "../lib/hooks";
import { describeError } from "../lib/errors";
import { principalKindLabel, roleDescription, roleLabel } from "../lib/labels";
import { formatDateTime } from "../lib/format";
import { EmptyState, ErrorState, QueryView } from "../components/states";
import { Modal } from "../components/modal";
import { Field } from "../components/field";

const LAST_MANAGER_MESSAGE =
  "A resource needs at least one manager. Give another user or a group the Manage role first, then remove this entry.";

function principalName(
  entry: { principalKind: PrincipalKind; principalId: string },
  groups: Group[],
  userId: string,
) {
  if (entry.principalKind === PrincipalKind.GROUP)
    return (
      groups.find((group) => group.id === entry.principalId)?.name ??
      entry.principalId
    );
  return entry.principalId === userId ? "You" : entry.principalId;
}

export function AccessView({
  resourceId,
  tenantId,
}: {
  resourceId: string;
  tenantId: string;
}) {
  const { session } = useSession();
  const entries = useQuery(rpc.listAclEntries, { resourceId });
  const groups = useQuery(rpc.listTenantGroups, { tenantId });
  const [editing, setEditing] = useState<AclEntry | "new" | undefined>();
  const [removing, setRemoving] = useState<AclEntry | undefined>();
  const groupList = groups.data?.groups ?? [];
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Access</h2>
          <p className="muted">
            Choose who can use this resource. People also need Managed CPA
            access in the tenant's Delcore permissions.
          </p>
        </div>
        <Button onClick={() => setEditing("new")}>Add people or groups</Button>
      </div>
      <dl className="role-legend">
        {[ResourceRole.MANAGE, ResourceRole.INVOKE].map((role) => (
          <div key={role}>
            <dt>{roleLabel(role)}</dt>
            <dd>{roleDescription(role)}</dd>
          </div>
        ))}
      </dl>
      <QueryView query={entries} loadingLabel="Loading access…">
        {(data) =>
          data.entries.length ? (
            <div className="table-scroll">
              <table>
                <caption className="sr-only">Access entries</caption>
                <thead>
                  <tr>
                    <th scope="col">Who</th>
                    <th scope="col">Type</th>
                    <th scope="col">Role</th>
                    <th scope="col">Added</th>
                    <th scope="col">
                      <span className="sr-only">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {data.entries.map((entry) => (
                    <tr key={`${entry.principalKind}:${entry.principalId}`}>
                      <td>{principalName(entry, groupList, session.userId)}</td>
                      <td>{principalKindLabel(entry.principalKind)}</td>
                      <td>{roleLabel(entry.role)}</td>
                      <td>{formatDateTime(entry.createTime)}</td>
                      <td className="row-actions">
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setEditing(entry)}
                        >
                          Change role
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setRemoving(entry)}
                        >
                          Remove
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <EmptyState
              title="No access entries"
              message="Add a user or a group so people can use this resource."
            />
          )
        }
      </QueryView>
      {editing && (
        <AclDialog
          resourceId={resourceId}
          entry={editing === "new" ? undefined : editing}
          groups={groupList}
          groupsError={groups.error}
          onClose={() => setEditing(undefined)}
        />
      )}
      {removing && (
        <RemoveAclDialog
          resourceId={resourceId}
          entry={removing}
          name={principalName(removing, groupList, session.userId)}
          onClose={() => setRemoving(undefined)}
        />
      )}
    </div>
  );
}

function AclDialog({
  resourceId,
  entry,
  groups,
  groupsError,
  onClose,
}: {
  resourceId: string;
  entry?: AclEntry;
  groups: Group[];
  groupsError: unknown;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const [kind, setKind] = useState(entry?.principalKind ?? PrincipalKind.GROUP);
  const [principalId, setPrincipalId] = useState(entry?.principalId ?? "");
  const [role, setRole] = useState(entry?.role ?? ResourceRole.INVOKE);
  const [error, setError] = useState<string | undefined>();
  const put = useMutation(rpc.putAclEntry, {
    onSuccess: async () => {
      await invalidate(rpc.listAclEntries, rpc.getResource, rpc.listResources);
      onClose();
    },
  });
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!principalId.trim()) {
      setError(
        kind === PrincipalKind.GROUP ? "Choose a group." : "Enter a user ID.",
      );
      return;
    }
    put.mutate({
      resourceId,
      principalKind: kind,
      principalId: principalId.trim(),
      role,
    });
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={entry ? "Change role" : "Add people or groups"}
      description={
        entry
          ? undefined
          : "Groups come from the tenant's Delcore groups. Users are identified by their Delcore user ID."
      }
      dismissible={!put.isPending}
    >
      <form className="form" onSubmit={submit} noValidate>
        {!entry && (
          <fieldset className="segmented">
            <legend>Add a</legend>
            {[PrincipalKind.GROUP, PrincipalKind.USER].map((option) => (
              <label key={option}>
                <input
                  type="radio"
                  name="principal-kind"
                  checked={kind === option}
                  onChange={() => {
                    setKind(option);
                    setPrincipalId("");
                    setError(undefined);
                  }}
                />
                {principalKindLabel(option)}
              </label>
            ))}
          </fieldset>
        )}
        {!entry && kind === PrincipalKind.GROUP && (
          <Field label="Group" error={error}>
            <NativeSelect
              value={principalId}
              onChange={(event) => setPrincipalId(event.target.value)}
            >
              <option value="">Choose a group</option>
              {groups.map((group) => (
                <option key={group.id} value={group.id}>
                  {group.name}
                </option>
              ))}
            </NativeSelect>
          </Field>
        )}
        {!entry && kind === PrincipalKind.GROUP && Boolean(groupsError) && (
          <ErrorState compact error={groupsError} />
        )}
        {!entry &&
          kind === PrincipalKind.GROUP &&
          !groupsError &&
          !groups.length && (
            <p className="muted small">
              This tenant has no groups yet. Create groups in Delcore, or add a
              user instead.
            </p>
          )}
        {!entry && kind === PrincipalKind.USER && (
          <Field label="Delcore user ID" error={error}>
            <Input
              autoFocus
              value={principalId}
              onChange={(event) => setPrincipalId(event.target.value)}
              spellCheck={false}
            />
          </Field>
        )}
        <Field label="Role" hint={roleDescription(role)}>
          <NativeSelect
            autoFocus={Boolean(entry)}
            value={String(role)}
            onChange={(event) =>
              setRole(Number(event.target.value) as ResourceRole)
            }
          >
            <option value={String(ResourceRole.INVOKE)}>Invoke</option>
            <option value={String(ResourceRole.MANAGE)}>Manage</option>
          </NativeSelect>
        </Field>
        {put.error && <ErrorState compact error={put.error} />}
        {put.error &&
          describeError(put.error).reason ===
            ErrorReason.FAILED_PRECONDITION && (
            <p className="muted small">{LAST_MANAGER_MESSAGE}</p>
          )}
        <div className="form-actions">
          <Button type="submit" disabled={put.isPending}>
            {put.isPending ? "Saving…" : entry ? "Save role" : "Add"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RemoveAclDialog({
  resourceId,
  entry,
  name,
  onClose,
}: {
  resourceId: string;
  entry: AclEntry;
  name: string;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const remove = useMutation(rpc.removeAclEntry, {
    onSuccess: async () => {
      await invalidate(rpc.listAclEntries, rpc.getResource, rpc.listResources);
      onClose();
    },
  });
  const lastManager =
    remove.error &&
    describeError(remove.error).reason === ErrorReason.FAILED_PRECONDITION;
  return (
    <Modal
      open
      onClose={onClose}
      title={`Remove ${name}?`}
      description="They lose this resource's role right away. Call keys they issued stop working unless they still have access."
      dismissible={!remove.isPending}
      footer={
        <>
          <Button
            variant="outline"
            onClick={onClose}
            disabled={remove.isPending}
          >
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={remove.isPending}
            onClick={() =>
              remove.mutate({
                resourceId,
                principalKind: entry.principalKind,
                principalId: entry.principalId,
              })
            }
          >
            {remove.isPending ? "Removing…" : "Remove"}
          </Button>
        </>
      }
    >
      {lastManager ? (
        <div className="notice notice-error" role="alert">
          <strong>Keep at least one manager</strong>
          <p>{LAST_MANAGER_MESSAGE}</p>
        </div>
      ) : (
        remove.error && <ErrorState compact error={remove.error} />
      )}
    </Modal>
  );
}
