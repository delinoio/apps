// SPDX-License-Identifier: Apache-2.0
import type { ReactNode } from "react";
import { Button } from "@packages/ui/components/button";
import { describeError, ErrorKind } from "../lib/errors";

export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <div className="state" role="status" aria-live="polite">
      <span className="spinner" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

export function EmptyState({
  title,
  message,
  action,
}: {
  title: string;
  message?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="state empty">
      <strong>{title}</strong>
      {message && <p>{message}</p>}
      {action}
    </div>
  );
}

/**
 * Shows a failed request in plain language. Permission failures explain the
 * missing access instead of offering a retry that cannot succeed.
 */
export function ErrorState({
  error,
  onRetry,
  permissionMessage,
  compact = false,
}: {
  error: unknown;
  onRetry?: () => void;
  permissionMessage?: string;
  compact?: boolean;
}) {
  const description = describeError(error);
  const denied = description.kind === ErrorKind.PermissionDenied;
  return (
    <div
      className={`notice ${denied ? "notice-muted" : "notice-error"} ${compact ? "compact" : ""}`}
      role="alert"
    >
      <strong>{description.title}</strong>
      <p>
        {denied && permissionMessage ? permissionMessage : description.message}
      </p>
      {description.reauthenticationUrl && (
        <p>
          <a href={description.reauthenticationUrl}>
            Set up two-step verification
          </a>
        </p>
      )}
      {description.traceId && (
        <p className="muted small">Trace ID: {description.traceId}</p>
      )}
      {onRetry && !denied && (
        <Button variant="outline" size="sm" onClick={onRetry}>
          Try again
        </Button>
      )}
    </div>
  );
}

/** Renders the loading, error and content states of one query. */
export function QueryView<T>({
  query,
  loadingLabel,
  permissionMessage,
  children,
}: {
  query: {
    data?: T;
    error: unknown;
    isPending: boolean;
    refetch: () => unknown;
  };
  loadingLabel?: string;
  permissionMessage?: string;
  children: (data: T) => ReactNode;
}) {
  if (query.isPending) return <Loading label={loadingLabel} />;
  if (query.error || query.data === undefined)
    return (
      <ErrorState
        error={query.error}
        onRetry={() => void query.refetch()}
        permissionMessage={permissionMessage}
      />
    );
  return <>{children(query.data)}</>;
}
