// SPDX-License-Identifier: Apache-2.0
import { useId, type ReactNode, type InputHTMLAttributes } from "react";
import { Button } from "@delino/ui/components/button";
import { Input } from "@delino/ui/components/input";
import { Badge } from "@delino/ui/components/badge";
import { Card } from "@delino/ui/components/card";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogTrigger,
  DialogClose,
} from "@delino/ui/components/dialog";
import { errorInfo, safeURL } from "./core";
export { Button, Badge, Card };
export function Field({
  label,
  hint,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { label: string; hint?: string }) {
  const id = useId();
  return (
    <label className="field" htmlFor={id}>
      <span>{label}</span>
      <Input
        id={id}
        aria-describedby={hint ? `${id}-hint` : undefined}
        {...props}
      />
      {hint && <small id={`${id}-hint`}>{hint}</small>}
    </label>
  );
}
export function Notice({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="notice" role="status">
      <strong>{title}</strong>
      {children && <div>{children}</div>}
    </div>
  );
}
export function Failure({
  error,
  retry,
}: {
  error: unknown;
  retry?: () => void;
}) {
  const e = errorInfo(error);
  return (
    <div className="notice error" role="alert">
      <strong>{e.denied ? "Permission denied" : "Request failed"}</strong>
      <p>{e.message}</p>
      <p>
        {e.denied
          ? "Ask a space manager for the required permission, or sign in with another account."
          : "Check your connection and try again. Uncertain publication must be reconciled before retrying."}
      </p>
      <small>Request ID: {e.requestId}</small>
      {retry && (
        <Button variant="outline" onClick={retry}>
          Retry
        </Button>
      )}
    </div>
  );
}
export function External({
  url,
  children,
  github = false,
}: {
  url: string;
  children: ReactNode;
  github?: boolean;
}) {
  const href = safeURL(url, github);
  return href ? (
    <a href={href} target="_blank" rel="noopener noreferrer">
      {children} <span aria-label="opens in a new tab">↗</span>
    </a>
  ) : (
    <span>Link unavailable</span>
  );
}
export function Confirm({
  label,
  description,
  onConfirm,
  disabled = false,
  pending = false,
}: {
  label: string;
  description: string;
  onConfirm: () => void;
  disabled?: boolean;
  pending?: boolean;
}) {
  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="outline" disabled={disabled || pending}>
          {label}
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogTitle>{label}?</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
        <div className="actions">
          <DialogClose asChild>
            <Button variant="outline">Keep current state</Button>
          </DialogClose>
          <DialogClose asChild>
            <Button onClick={onConfirm}>{label}</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  );
}
export function Pager({
  next,
  previous,
  busy,
}: {
  next?: () => void;
  previous?: () => void;
  busy?: boolean;
}) {
  return (
    <div className="actions">
      <Button variant="outline" disabled={!previous || busy} onClick={previous}>
        Previous page
      </Button>
      <Button variant="outline" disabled={!next || busy} onClick={next}>
        Next page
      </Button>
    </div>
  );
}
