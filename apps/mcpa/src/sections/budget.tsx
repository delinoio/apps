// SPDX-License-Identifier: Apache-2.0
import { useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import type { Budget } from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc, useInvalidate } from "../lib/hooks";
import { describeError, ErrorKind } from "../lib/errors";
import { formatDateTime, formatUsd, normalizeUsdInput } from "../lib/format";
import { EmptyState, ErrorState, Loading } from "../components/states";
import { Modal } from "../components/modal";
import { Field } from "../components/field";

function BudgetSummary({ budget }: { budget: Budget }) {
  const limit = budget.limitUsd ? Number(budget.limitUsd) : undefined;
  const spent = Number(budget.spentUsd || "0");
  // A zero limit is exhausted at once, but dividing by it is meaningless, so
  // the server's exhausted flag decides the full meter.
  const ratio = budget.exhausted
    ? 1
    : limit && limit > 0
      ? Math.min(spent / limit, 1)
      : 0;
  return (
    <div className="budget">
      <dl className="stats">
        <div>
          <dt>Limit this period</dt>
          <dd>{budget.limitUsd ? formatUsd(budget.limitUsd) : "No limit"}</dd>
        </div>
        <div>
          <dt>Estimated spend</dt>
          <dd>{formatUsd(budget.spentUsd)}</dd>
        </div>
        <div>
          <dt>Billing period</dt>
          <dd className="small">
            {formatDateTime(budget.periodStart)} –{" "}
            {formatDateTime(budget.periodEnd)}
          </dd>
        </div>
      </dl>
      {limit !== undefined && (
        <div
          className="meter"
          role="meter"
          aria-label="Budget used"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(ratio * 100)}
        >
          <span
            style={{ width: `${ratio * 100}%` }}
            data-full={budget.exhausted || undefined}
          />
        </div>
      )}
      {budget.exhausted && (
        <div className="notice notice-error" role="status">
          <strong>Budget reached</strong>
          <p>
            New model calls are rejected until the next billing period or until
            you raise the limit.
          </p>
        </div>
      )}
      <p className="muted small">
        Spend is estimated from measured traffic at USD 0.07 per GiB. Calls
        already in progress finish, so spend can go slightly over the limit.
        Delcore issues the final bill.
      </p>
    </div>
  );
}

function BudgetDialog({
  title,
  current,
  pending,
  error,
  onSave,
  onClose,
}: {
  title: string;
  current: string;
  pending: boolean;
  error: unknown;
  onSave: (limitUsd: string) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(current);
  const [invalid, setInvalid] = useState(false);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    // An untouched prefilled limit is already canonical API output, so send it
    // back as is. Typed input is limited to 12 integer and 6 fraction digits,
    // which is narrower than what the server stores.
    const normalized =
      current && value === current ? current : normalizeUsdInput(value);
    if (!normalized) {
      setInvalid(true);
      return;
    }
    onSave(normalized);
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      description="New calls are rejected once estimated spend in the billing period reaches this amount."
      dismissible={!pending}
    >
      <form className="form" onSubmit={submit} noValidate>
        <Field
          label="Monthly limit (USD)"
          error={invalid ? "Enter an amount such as 25 or 25.50." : undefined}
        >
          <Input
            autoFocus
            inputMode="decimal"
            value={value}
            onChange={(event) => {
              setValue(event.target.value);
              setInvalid(false);
            }}
            placeholder="25.00"
          />
        </Field>
        {Boolean(error) && <ErrorState compact error={error} />}
        <div className="form-actions">
          <Button type="submit" disabled={pending}>
            {pending ? "Saving…" : "Save limit"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function BudgetPanel({
  heading,
  description,
  query,
  save,
  canEdit,
  deniedMessage,
}: {
  heading: string;
  description: string;
  query: {
    data?: { budget?: Budget };
    error: unknown;
    isPending: boolean;
    refetch: () => unknown;
  };
  save: {
    run: (limitUsd: string) => Promise<unknown>;
    pending: boolean;
    error: unknown;
    reset: () => void;
  };
  canEdit: boolean;
  deniedMessage: string;
}) {
  const [editing, setEditing] = useState(false);
  const close = () => {
    setEditing(false);
    save.reset();
  };
  const apply = (limit: string) =>
    void save.run(limit).then(close, () => undefined);
  let body;
  if (query.isPending) body = <Loading label="Loading budget…" />;
  else if (query.error) {
    body =
      describeError(query.error).kind === ErrorKind.PermissionDenied ? (
        <EmptyState
          title="You can't view this budget"
          message={deniedMessage}
        />
      ) : (
        <ErrorState error={query.error} onRetry={() => void query.refetch()} />
      );
  } else {
    const budget = query.data?.budget;
    body = budget ? (
      <BudgetSummary budget={budget} />
    ) : (
      <EmptyState title="No budget information" />
    );
  }
  const budget = query.data?.budget;
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>{heading}</h2>
          <p className="muted">{description}</p>
        </div>
        {canEdit && budget && (
          <div className="actions">
            {budget.limitUsd && (
              <Button
                variant="outline"
                disabled={save.pending}
                onClick={() => void save.run("").catch(() => undefined)}
              >
                Remove limit
              </Button>
            )}
            <Button onClick={() => setEditing(true)}>
              {budget.limitUsd ? "Change limit" : "Set a limit"}
            </Button>
          </div>
        )}
      </div>
      {!editing && Boolean(save.error) && (
        <ErrorState compact error={save.error} />
      )}
      {body}
      {editing && (
        <BudgetDialog
          title={budget?.limitUsd ? "Change the limit" : "Set a limit"}
          current={budget?.limitUsd ?? ""}
          pending={save.pending}
          error={save.error}
          onSave={apply}
          onClose={close}
        />
      )}
    </div>
  );
}

export function ResourceBudget({ resourceId }: { resourceId: string }) {
  const invalidate = useInvalidate();
  const query = useQuery(rpc.getResourceBudget, { resourceId });
  const mutation = useMutation(rpc.setResourceBudget, {
    onSuccess: () => invalidate(rpc.getResourceBudget),
  });
  return (
    <BudgetPanel
      heading="Resource budget"
      description="An optional spending limit for this resource in each Delcore billing period."
      query={query}
      canEdit
      deniedMessage="Only resource managers can view this budget."
      save={{
        run: (limitUsd) => mutation.mutateAsync({ resourceId, limitUsd }),
        pending: mutation.isPending,
        error: mutation.error,
        reset: mutation.reset,
      }}
    />
  );
}

export function TenantBudget({ tenantId }: { tenantId: string }) {
  const invalidate = useInvalidate();
  const query = useQuery(rpc.getTenantBudget, { tenantId }, { retry: false });
  const mutation = useMutation(rpc.setTenantBudget, {
    onSuccess: () => invalidate(rpc.getTenantBudget),
  });
  return (
    <BudgetPanel
      heading="Tenant budget"
      description="An optional spending limit for all Managed CPA resources in this tenant in each Delcore billing period."
      query={query}
      canEdit
      deniedMessage="The tenant budget is available to members with the Managed CPA budget permission in Delcore. Ask a tenant admin if you need it."
      save={{
        run: (limitUsd) => mutation.mutateAsync({ tenantId, limitUsd }),
        pending: mutation.isPending,
        error: mutation.error,
        reset: mutation.reset,
      }}
    />
  );
}
