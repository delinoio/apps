// SPDX-License-Identifier: Apache-2.0
import { useState, type FormEvent } from "react";
import { useMutation } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { LoadBalancing, type Resource } from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc, useInvalidate } from "../lib/hooks";
import { ALLOWLIST_MAX_ENTRIES, modelNameError } from "../lib/validate";
import { ErrorState } from "../components/states";
import { Field } from "../components/field";

export function RoutingView({ resource }: { resource: Resource }) {
  const invalidate = useInvalidate();
  const saved = resource.routing?.modelAllowlist ?? [];
  const [models, setModels] = useState<string[]>(saved);
  const [draft, setDraft] = useState("");
  const [draftError, setDraftError] = useState<string | undefined>();
  const [savedNotice, setSavedNotice] = useState(false);
  const update = useMutation(rpc.updateRouting, {
    onSuccess: async () => {
      setSavedNotice(true);
      await invalidate(rpc.getResource, rpc.listResources);
    },
  });
  const dirty =
    models.length !== saved.length ||
    models.some((model, index) => model !== saved[index]);
  const add = (event: FormEvent) => {
    event.preventDefault();
    const model = draft.trim();
    const invalid = modelNameError(model);
    if (invalid) return setDraftError(invalid);
    if (models.includes(model))
      return setDraftError("This model is already on the list.");
    if (models.length >= ALLOWLIST_MAX_ENTRIES)
      return setDraftError(
        `The list can have up to ${ALLOWLIST_MAX_ENTRIES} models.`,
      );
    setModels([...models, model]);
    setDraft("");
    setDraftError(undefined);
    setSavedNotice(false);
  };
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Routing</h2>
          <p className="muted">How calls are sent to your provider accounts.</p>
        </div>
      </div>
      <div className="card-block">
        <h3>Load balancing</h3>
        <p>
          <strong>Round-robin.</strong> Calls rotate across this resource's
          accounts for the same provider. A failed call is returned to the
          caller and isn't retried on another account.
        </p>
      </div>
      <div className="card-block">
        <h3>Model allowlist</h3>
        <p className="muted">
          {models.length
            ? "Only the models on this list can be called. Requests for other models are rejected before they reach a provider."
            : "The list is empty, so every model your credentials can serve is allowed."}
        </p>
        <form className="inline-form" onSubmit={add} noValidate>
          <Field label="Model name" error={draftError}>
            <Input
              value={draft}
              onChange={(event) => {
                setDraft(event.target.value);
                setDraftError(undefined);
              }}
              placeholder="Exact model name"
            />
          </Field>
          <Button type="submit" variant="outline">
            Add
          </Button>
        </form>
        {models.length > 0 && (
          <ul className="chips" aria-label="Allowed models">
            {models.map((model) => (
              <li key={model} className="chip">
                <span className="mono">{model}</span>
                <button
                  type="button"
                  className="chip-remove"
                  aria-label={`Remove ${model}`}
                  onClick={() => {
                    setModels(models.filter((item) => item !== model));
                    setSavedNotice(false);
                  }}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
        {update.error && <ErrorState compact error={update.error} />}
        {savedNotice && !dirty && (
          <p className="success small" role="status">
            Routing saved.
          </p>
        )}
        <div className="form-actions">
          {dirty && (
            <Button
              variant="ghost"
              onClick={() => {
                setModels(saved);
                update.reset();
              }}
            >
              Discard changes
            </Button>
          )}
          <Button
            disabled={!dirty || update.isPending}
            onClick={() =>
              update.mutate({
                resourceId: resource.id,
                routing: {
                  loadBalancing: LoadBalancing.ROUND_ROBIN,
                  modelAllowlist: models,
                },
              })
            }
          >
            {update.isPending ? "Saving…" : "Save routing"}
          </Button>
        </div>
      </div>
    </div>
  );
}
