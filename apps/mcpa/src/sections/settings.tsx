// SPDX-License-Identifier: Apache-2.0
import { useState, type FormEvent } from "react";
import { useMutation } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { Textarea } from "@packages/ui/components/textarea";
import type { Resource } from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc, useInvalidate } from "../lib/hooks";
import { paths, useRouter } from "../lib/router";
import { descriptionError, nameError } from "../lib/validate";
import { ErrorState } from "../components/states";
import { Modal } from "../components/modal";
import { Field } from "../components/field";

export function SettingsView({ resource }: { resource: Resource }) {
  const invalidate = useInvalidate();
  const [name, setName] = useState(resource.name);
  const [description, setDescription] = useState(resource.description);
  const [errors, setErrors] = useState<{ name?: string; description?: string }>(
    {},
  );
  const [deleting, setDeleting] = useState(false);
  const [saved, setSaved] = useState(false);
  const update = useMutation(rpc.updateResource, {
    onSuccess: async () => {
      setSaved(true);
      await invalidate(rpc.getResource, rpc.listResources);
    },
  });
  const dirty =
    name.trim() !== resource.name ||
    description.trim() !== resource.description;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const next = {
      name: nameError(name),
      description: descriptionError(description),
    };
    setErrors(next);
    if (next.name || next.description) return;
    update.mutate({
      resourceId: resource.id,
      name: name.trim(),
      description: description.trim(),
    });
  };
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Settings</h2>
        </div>
      </div>
      <form className="form card-block" onSubmit={submit} noValidate>
        <Field label="Name" error={errors.name}>
          <Input
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              setSaved(false);
            }}
          />
        </Field>
        <Field label="Description (optional)" error={errors.description}>
          <Textarea
            value={description}
            onChange={(event) => {
              setDescription(event.target.value);
              setSaved(false);
            }}
          />
        </Field>
        {update.error && <ErrorState compact error={update.error} />}
        {saved && !dirty && (
          <p className="success small" role="status">
            Changes saved.
          </p>
        )}
        <div className="form-actions">
          <Button type="submit" disabled={!dirty || update.isPending}>
            {update.isPending ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </form>
      <div className="card-block danger-zone">
        <h3>Delete this resource</h3>
        <p className="muted">
          Its call keys and credentials are removed immediately and its endpoint
          stops working. Usage, error and audit records stay until their
          retention period ends.
        </p>
        <div className="form-actions">
          <Button variant="destructive" onClick={() => setDeleting(true)}>
            Delete resource
          </Button>
        </div>
      </div>
      {deleting && (
        <DeleteResourceDialog
          resource={resource}
          onClose={() => setDeleting(false)}
        />
      )}
    </div>
  );
}

function DeleteResourceDialog({
  resource,
  onClose,
}: {
  resource: Resource;
  onClose: () => void;
}) {
  const { navigate } = useRouter();
  const invalidate = useInvalidate();
  const [confirm, setConfirm] = useState("");
  const remove = useMutation(rpc.deleteResource, {
    onSuccess: async () => {
      await invalidate(rpc.listResources);
      onClose();
      navigate(paths.tenant(resource.tenantId));
    },
  });
  const matches = confirm === resource.name;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!matches) return;
    remove.mutate({ resourceId: resource.id, confirmName: confirm });
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={`Delete ${resource.name}?`}
      description="Call keys and credentials are removed immediately. Records stay until their retention ends. This can't be undone."
      dismissible={!remove.isPending}
    >
      <form className="form" onSubmit={submit} noValidate>
        <Field label={`Type ${resource.name} to confirm`}>
          <Input
            autoFocus
            value={confirm}
            autoComplete="off"
            onChange={(event) => setConfirm(event.target.value)}
          />
        </Field>
        {remove.error && <ErrorState compact error={remove.error} />}
        <div className="form-actions">
          <Button variant="outline" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="destructive"
            disabled={!matches || remove.isPending}
          >
            {remove.isPending ? "Deleting…" : "Delete resource"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
