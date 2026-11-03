// SPDX-License-Identifier: Apache-2.0
import { useRef, useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { NativeSelect } from "@packages/ui/components/native-select";
import { ResourceRole, type CallKey } from "@repo/protos/mcpa/v1/mcpa_pb";
import { useSession } from "../app";
import { rpc, useInvalidate, useSecretMutation } from "../lib/hooks";
import { useCursorPager } from "../lib/pager";
import { nameError } from "../lib/validate";
import { formatDateTime, toDate } from "../lib/format";
import { uuidv7 } from "../lib/uuid";
import { canManage } from "../lib/roles";
import { EmptyState, ErrorState, QueryView } from "../components/states";
import { Modal } from "../components/modal";
import { Field } from "../components/field";
import { PagerControls } from "../components/pager";
import { CopyButton } from "../components/copy-button";

export enum KeyStatus {
  Active = "active",
  Expired = "expired",
  Revoked = "revoked",
}

export function keyStatus(key: CallKey, now = Date.now()): KeyStatus {
  if (key.revokeTime) return KeyStatus.Revoked;
  const expires = toDate(key.expireTime);
  if (expires && expires.getTime() <= now) return KeyStatus.Expired;
  return KeyStatus.Active;
}

const keyStatusLabels: Record<KeyStatus, string> = {
  [KeyStatus.Active]: "Active",
  [KeyStatus.Expired]: "Expired",
  [KeyStatus.Revoked]: "Revoked",
};

enum Expiry {
  Never = "never",
  Days30 = "30",
  Days90 = "90",
  Year = "365",
  Custom = "custom",
}

const expiryLabels: Record<Expiry, string> = {
  [Expiry.Never]: "No expiry",
  [Expiry.Days30]: "In 30 days",
  [Expiry.Days90]: "In 90 days",
  [Expiry.Year]: "In 1 year",
  [Expiry.Custom]: "On a date I choose",
};

function expiryDate(expiry: Expiry, custom: string): Date | undefined | null {
  if (expiry === Expiry.Never) return undefined;
  if (expiry === Expiry.Custom) {
    if (!custom) return null;
    // The key stops working at the end of the chosen local day.
    const date = new Date(`${custom}T23:59:59`);
    return Number.isNaN(date.getTime()) || date.getTime() <= Date.now()
      ? null
      : date;
  }
  return new Date(Date.now() + Number(expiry) * 86_400_000);
}

export function KeysView({
  resourceId,
  role,
}: {
  resourceId: string;
  role: ResourceRole;
}) {
  const { session } = useSession();
  const pager = useCursorPager();
  const [issuing, setIssuing] = useState(false);
  const [revoking, setRevoking] = useState<CallKey | undefined>();
  const keys = useQuery(rpc.listCallKeys, {
    resourceId,
    page: { size: 50, afterId: pager.afterId },
  });
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Call keys</h2>
          <p className="muted">
            {canManage(role)
              ? "Applications use call keys to call models through this resource. You can see and revoke every key."
              : "Applications use call keys to call models through this resource. You can see and revoke the keys you issued."}
          </p>
        </div>
        <Button onClick={() => setIssuing(true)}>Issue call key</Button>
      </div>
      <QueryView query={keys} loadingLabel="Loading call keys…">
        {(data) =>
          data.callKeys.length ? (
            <>
              <div className="table-scroll">
                <table>
                  <caption className="sr-only">Call keys</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">Key</th>
                      <th scope="col">Issued by</th>
                      <th scope="col">Expires</th>
                      <th scope="col">Last used</th>
                      <th scope="col">Status</th>
                      <th scope="col">
                        <span className="sr-only">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.callKeys.map((key) => {
                      const status = keyStatus(key);
                      return (
                        <tr key={key.id}>
                          <td>
                            {key.name || "Untitled key"}
                            <span className="muted small block">
                              Created {formatDateTime(key.createTime)}
                            </span>
                          </td>
                          <td className="mono">{key.displayPrefix}</td>
                          <td>
                            {key.issuerUserId === session.userId
                              ? "You"
                              : key.issuerUserId}
                          </td>
                          <td>{formatDateTime(key.expireTime, "Never")}</td>
                          <td>
                            {formatDateTime(key.lastUsedTime, "Never used")}
                          </td>
                          <td>
                            <span
                              className={`status status-${status === KeyStatus.Active ? "good" : "neutral"}`}
                            >
                              {keyStatusLabels[status]}
                            </span>
                            {key.revokeTime && (
                              <span className="muted small block">
                                {formatDateTime(key.revokeTime)}
                              </span>
                            )}
                          </td>
                          <td className="row-actions">
                            {status !== KeyStatus.Revoked && (
                              <Button
                                variant="outline"
                                size="sm"
                                onClick={() => setRevoking(key)}
                              >
                                Revoke
                              </Button>
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
              title="No call keys yet"
              message="Issue a call key to let an application call models through this resource."
              action={
                <Button onClick={() => setIssuing(true)}>Issue call key</Button>
              }
            />
          )
        }
      </QueryView>
      {issuing && (
        <IssueKeyDialog
          resourceId={resourceId}
          onClose={() => setIssuing(false)}
          onRevoke={(key) => {
            setIssuing(false);
            setRevoking(key);
          }}
        />
      )}
      {revoking && (
        <RevokeKeyDialog
          resourceId={resourceId}
          callKey={revoking}
          onClose={() => setRevoking(undefined)}
        />
      )}
    </div>
  );
}

/** The expiry sent with the dialog's request key, and the choice behind it. */
type SubmittedExpiry = { choice: string; expireAt: Date | undefined };

function IssueKeyDialog({
  resourceId,
  onClose,
  onRevoke,
}: {
  resourceId: string;
  onClose: () => void;
  onRevoke: (key: CallKey) => void;
}) {
  const invalidate = useInvalidate();
  // One idempotency key per opened form, so a retried submit is safe.
  const [requestKey] = useState(() => uuidv7());
  // The server fingerprints the request key with the exact expiry. A retry
  // with an unchanged expiry choice resends the first timestamp instead of
  // recomputing it from the current time, so the server replays the result.
  const submitted = useRef<SubmittedExpiry | undefined>(undefined);
  const [name, setName] = useState("");
  const [expiry, setExpiry] = useState(Expiry.Days90);
  const [custom, setCustom] = useState("");
  const [errors, setErrors] = useState<{ name?: string; expiry?: string }>({});
  const issue = useSecretMutation(rpc.issueCallKey, {
    onSuccess: () => invalidate(rpc.listCallKeys),
  });
  // The secret exists only in this dialog's mutation state. Closing or
  // unmounting the dialog discards the mutation from the query client too,
  // so the secret cannot be shown again.
  const secret = issue.data?.secret;
  const close = () => {
    issue.discard();
    onClose();
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const choice = expiry === Expiry.Custom ? `${expiry}:${custom}` : expiry;
    const expireAt =
      submitted.current?.choice === choice
        ? submitted.current.expireAt
        : expiryDate(expiry, custom);
    const next = {
      name: name.trim()
        ? nameError(name)
        : "Enter a name so you can recognize this key.",
      expiry: expireAt === null ? "Choose a date in the future." : undefined,
    };
    setErrors(next);
    if (next.name || next.expiry) return;
    submitted.current = { choice, expireAt: expireAt ?? undefined };
    issue.mutate({
      requestKey,
      resourceId,
      name: name.trim(),
      expireTime: expireAt ? timestampFromDate(expireAt) : undefined,
    });
  };
  // A replayed request key returns the key without its secret: an earlier
  // attempt created it, but its response did not arrive.
  const replayed = issue.isSuccess && !secret ? issue.data?.callKey : undefined;
  if (replayed)
    return (
      <Modal
        open
        onClose={close}
        title="Your call key was created"
        description="An earlier attempt created this key, but its response didn't arrive. The full key can't be shown again."
        footer={
          <>
            <Button variant="outline" onClick={close}>
              Close
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                issue.discard();
                onRevoke(replayed);
              }}
            >
              Revoke key
            </Button>
          </>
        }
      >
        <div className="notice notice-warning" role="note">
          <strong>Revoke it and issue a new key</strong>
          <p>
            Revoke <span className="mono">{replayed.displayPrefix}</span> (
            {replayed.name || "Untitled key"}) so it can't be used, then issue a
            new key you can copy.
          </p>
        </div>
      </Modal>
    );
  if (secret)
    return (
      <Modal
        open
        onClose={close}
        title="Copy your call key"
        description="This is the only time the full key is shown. Store it somewhere safe, such as a secret manager."
        footer={<Button onClick={close}>I've copied it</Button>}
      >
        <div className="secret">
          <Input
            readOnly
            aria-label="Call key"
            value={secret}
            className="mono"
            onFocus={(event) => event.currentTarget.select()}
          />
          <CopyButton value={secret} label="Copy key" variant="default" />
        </div>
        <div className="notice notice-warning" role="note">
          <strong>You won't see this key again</strong>
          <p>
            After you close this window, only its prefix{" "}
            <span className="mono">{issue.data?.callKey?.displayPrefix}</span>{" "}
            is shown. If you lose the key, revoke it and issue a new one.
          </p>
        </div>
      </Modal>
    );
  return (
    <Modal
      open
      onClose={close}
      title="Issue a call key"
      description="Applications send this key to call models through this resource."
      dismissible={!issue.isPending}
    >
      <form className="form" onSubmit={submit} noValidate>
        <Field label="Name" error={errors.name}>
          <Input
            autoFocus
            value={name}
            maxLength={100}
            onChange={(event) => setName(event.target.value)}
            placeholder="For example, Production backend"
          />
        </Field>
        <Field
          label="Expires"
          error={expiry !== Expiry.Custom ? errors.expiry : undefined}
        >
          <NativeSelect
            value={expiry}
            onChange={(event) => setExpiry(event.target.value as Expiry)}
          >
            {Object.values(Expiry).map((value) => (
              <option key={value} value={value}>
                {expiryLabels[value]}
              </option>
            ))}
          </NativeSelect>
        </Field>
        {expiry === Expiry.Custom && (
          <Field label="Expiry date" error={errors.expiry}>
            <Input
              type="date"
              value={custom}
              onChange={(event) => setCustom(event.target.value)}
            />
          </Field>
        )}
        {issue.error && <ErrorState compact error={issue.error} />}
        <div className="form-actions">
          <Button type="submit" disabled={issue.isPending}>
            {issue.isPending ? "Issuing…" : "Issue key"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RevokeKeyDialog({
  resourceId,
  callKey,
  onClose,
}: {
  resourceId: string;
  callKey: CallKey;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const revoke = useMutation(rpc.revokeCallKey, {
    onSuccess: async () => {
      await invalidate(rpc.listCallKeys);
      onClose();
    },
  });
  return (
    <Modal
      open
      onClose={onClose}
      title={`Revoke ${callKey.name || callKey.displayPrefix}?`}
      description="Applications using this key stop working right away. This can't be undone."
      dismissible={!revoke.isPending}
      footer={
        <>
          <Button
            variant="outline"
            onClick={onClose}
            disabled={revoke.isPending}
          >
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate({ resourceId, callKeyId: callKey.id })}
          >
            {revoke.isPending ? "Revoking…" : "Revoke key"}
          </Button>
        </>
      }
    >
      {revoke.error && <ErrorState compact error={revoke.error} />}
    </Modal>
  );
}
