// SPDX-License-Identifier: Apache-2.0
import { useEffect, useReducer, useState, type FormEvent } from "react";
import { useMutation, useQuery } from "@connectrpc/connect-query";
import { Button } from "@packages/ui/components/button";
import { Input } from "@packages/ui/components/input";
import { NativeSelect } from "@packages/ui/components/native-select";
import { Textarea } from "@packages/ui/components/textarea";
import {
  CredentialKind,
  Provider,
  type Credential,
} from "@repo/protos/mcpa/v1/mcpa_pb";
import { rpc, useInvalidate, useSecretMutation } from "../lib/hooks";
import {
  apiKeyProviders,
  credentialKindLabel,
  oauthProviders,
  providerLabel,
} from "../lib/labels";
import { formatDateTime } from "../lib/format";
import {
  initialOAuthState,
  isCallbackUrl,
  OAuthEventType,
  oauthReducer,
  OAuthStep,
} from "../lib/oauth";
import {
  isHttpsUrl,
  modelNameError,
  nameError,
  parseList,
} from "../lib/validate";
import { EmptyState, ErrorState, QueryView } from "../components/states";
import { Modal, useFocusOnMount } from "../components/modal";
import { Field } from "../components/field";
import { CopyButton } from "../components/copy-button";

/** How often a device-code flow is polled while it waits for approval. */
export const DEVICE_POLL_MS = 3000;

enum Dialog {
  None = "none",
  OAuth = "oauth",
  ApiKey = "api-key",
}

export function CredentialsView({ resourceId }: { resourceId: string }) {
  const [dialog, setDialog] = useState(Dialog.None);
  const [removing, setRemoving] = useState<Credential | undefined>();
  const credentials = useQuery(rpc.listCredentials, { resourceId });
  const close = () => setDialog(Dialog.None);
  return (
    <div className="section-body">
      <div className="section-head">
        <div>
          <h2>Credentials</h2>
          <p className="muted">
            Your own provider accounts and API keys. Calls are routed to them,
            and your provider bills you directly. Secrets are encrypted and
            never shown again.
          </p>
        </div>
        <div className="actions">
          <Button variant="outline" onClick={() => setDialog(Dialog.ApiKey)}>
            Add API key
          </Button>
          <Button onClick={() => setDialog(Dialog.OAuth)}>
            Connect account
          </Button>
        </div>
      </div>
      <QueryView query={credentials} loadingLabel="Loading credentials…">
        {(data) =>
          data.credentials.length ? (
            <ul className="card-list">
              {data.credentials.map((credential) => (
                <li key={credential.id} className="card-block">
                  <div className="endpoint-head">
                    <div>
                      <h3>
                        {credential.name || providerLabel(credential.provider)}
                      </h3>
                      <p className="muted small">
                        {providerLabel(credential.provider)} ·{" "}
                        {credentialKindLabel(credential.kind)}
                      </p>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setRemoving(credential)}
                    >
                      Remove
                    </Button>
                  </div>
                  <dl className="details">
                    {credential.kind === CredentialKind.OAUTH ? (
                      <div>
                        <dt>Account</dt>
                        <dd>
                          {credential.accountLabel || "Connected account"}
                        </dd>
                      </div>
                    ) : (
                      <div>
                        <dt>Key</dt>
                        <dd className="mono">
                          {credential.secretHint
                            ? `Ends in ${credential.secretHint}`
                            : "Stored"}
                        </dd>
                      </div>
                    )}
                    {credential.baseUrl && (
                      <div>
                        <dt>Base URL</dt>
                        <dd className="mono">{credential.baseUrl}</dd>
                      </div>
                    )}
                    {credential.models.length > 0 && (
                      <div>
                        <dt>Models</dt>
                        <dd className="mono">{credential.models.join(", ")}</dd>
                      </div>
                    )}
                    <div>
                      <dt>Added</dt>
                      <dd>{formatDateTime(credential.createTime)}</dd>
                    </div>
                  </dl>
                </li>
              ))}
            </ul>
          ) : (
            <EmptyState
              title="No credentials yet"
              message="Connect a provider account or add an API key so calls can reach a provider."
              action={
                <Button onClick={() => setDialog(Dialog.OAuth)}>
                  Connect account
                </Button>
              }
            />
          )
        }
      </QueryView>
      {dialog === Dialog.OAuth && (
        <ConnectAccountDialog resourceId={resourceId} onClose={close} />
      )}
      {dialog === Dialog.ApiKey && (
        <ApiKeyDialog resourceId={resourceId} onClose={close} />
      )}
      {removing && (
        <RemoveCredentialDialog
          resourceId={resourceId}
          credential={removing}
          onClose={() => setRemoving(undefined)}
        />
      )}
    </div>
  );
}

export function ConnectAccountDialog({
  resourceId,
  onClose,
}: {
  resourceId: string;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const [state, dispatch] = useReducer(oauthReducer, initialOAuthState);
  const [provider, setProvider] = useState(oauthProviders[0].provider);
  const [name, setName] = useState("");
  const [callbackUrl, setCallbackUrl] = useState("");
  const [fieldError, setFieldError] = useState<string | undefined>();
  const start = useMutation(rpc.startProviderOAuth);
  const complete = useMutation(rpc.completeProviderOAuth);
  const flow = state.flow;
  const poll = useQuery(
    rpc.getProviderOAuthFlow,
    { resourceId, flowId: flow?.id ?? "" },
    {
      enabled: state.step === OAuthStep.Device && Boolean(flow?.id),
      refetchInterval: DEVICE_POLL_MS,
      retry: false,
      gcTime: 0,
    },
  );
  useEffect(() => {
    if (poll.data?.flow)
      dispatch({ type: OAuthEventType.FlowUpdated, flow: poll.data.flow });
  }, [poll.data]);
  useEffect(() => {
    if (poll.error)
      dispatch({ type: OAuthEventType.RequestFailed, error: poll.error });
  }, [poll.error]);
  useEffect(() => {
    if (state.step === OAuthStep.Completed)
      void invalidate(rpc.listCredentials);
  }, [state.step, invalidate]);

  const begin = async (event: FormEvent) => {
    event.preventDefault();
    const error = nameError(name);
    setFieldError(error);
    if (error) return;
    dispatch({ type: OAuthEventType.Start });
    try {
      const response = await start.mutateAsync({
        resourceId,
        provider,
        name: name.trim(),
      });
      if (!response.flow) throw new Error("The flow was not started.");
      dispatch({ type: OAuthEventType.FlowUpdated, flow: response.flow });
    } catch (error) {
      dispatch({ type: OAuthEventType.RequestFailed, error });
    }
  };
  const finish = async (event: FormEvent) => {
    event.preventDefault();
    if (!flow) return;
    if (!isCallbackUrl(callbackUrl)) {
      setFieldError(
        "Paste the full address, starting with http:// or https://.",
      );
      return;
    }
    setFieldError(undefined);
    dispatch({ type: OAuthEventType.Submit });
    try {
      const response = await complete.mutateAsync({
        resourceId,
        flowId: flow.id,
        callbackUrl: callbackUrl.trim(),
      });
      if (!response.flow) throw new Error("The flow was not updated.");
      dispatch({ type: OAuthEventType.FlowUpdated, flow: response.flow });
    } catch (error) {
      dispatch({ type: OAuthEventType.RequestFailed, error });
    }
  };
  const restart = () => {
    setCallbackUrl("");
    setFieldError(undefined);
    start.reset();
    complete.reset();
    dispatch({ type: OAuthEventType.Reset });
  };
  const busy =
    state.step === OAuthStep.Starting || state.step === OAuthStep.Submitting;

  let body;
  switch (state.step) {
    case OAuthStep.Choose:
    case OAuthStep.Starting:
      body = (
        <form className="form" onSubmit={begin} noValidate>
          <Field label="Provider">
            <NativeSelect
              autoFocus
              value={String(provider)}
              onChange={(event) =>
                setProvider(Number(event.target.value) as Provider)
              }
            >
              {oauthProviders.map((option) => (
                <option key={option.provider} value={String(option.provider)}>
                  {option.label}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field
            label="Name"
            error={fieldError}
            hint="Helps you tell accounts apart."
          >
            <Input
              value={name}
              maxLength={100}
              onChange={(event) => setName(event.target.value)}
              placeholder="For example, Team Codex account"
            />
          </Field>
          {Boolean(state.error) && <ErrorState compact error={state.error} />}
          <div className="form-actions">
            <Button type="submit" disabled={busy}>
              {busy ? "Starting…" : "Continue"}
            </Button>
          </div>
        </form>
      );
      break;
    case OAuthStep.Authorize:
    case OAuthStep.Submitting:
      body = (
        <form className="form" onSubmit={finish} noValidate>
          <ol className="steps">
            <li>
              <a
                href={flow?.authorizationUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="strong-link"
              >
                Open the {providerLabel(flow?.provider ?? provider)} sign-in
                page
              </a>{" "}
              and approve access.
            </li>
            <li>
              When the browser shows the final page, copy the full address from
              the address bar, even if the page shows an error.
            </li>
          </ol>
          <Field label="Final page address" error={fieldError}>
            <CallbackUrlInput value={callbackUrl} onChange={setCallbackUrl} />
          </Field>
          {Boolean(state.error) && <ErrorState compact error={state.error} />}
          {flow?.expireTime && (
            <p className="muted small">
              This request expires {formatDateTime(flow.expireTime)}.
            </p>
          )}
          <div className="form-actions">
            <Button type="submit" disabled={busy}>
              {busy ? "Connecting…" : "Finish connecting"}
            </Button>
          </div>
        </form>
      );
      break;
    case OAuthStep.Device:
      body = (
        <div className="form">
          <ol className="steps">
            <li>
              Open{" "}
              <a
                href={flow?.verificationUri}
                target="_blank"
                rel="noopener noreferrer"
                className="strong-link"
              >
                {flow?.verificationUri}
              </a>
              .
            </li>
            <li>Enter this code and approve access.</li>
          </ol>
          <div className="device-code">
            <span className="mono" aria-label="Device code">
              {flow?.userCode}
            </span>
            <CopyButton value={flow?.userCode ?? ""} label="Copy code" />
          </div>
          <p className="state" role="status" aria-live="polite">
            <span className="spinner" aria-hidden="true" /> Waiting for you to
            approve…
          </p>
          {Boolean(state.error) && <ErrorState compact error={state.error} />}
        </div>
      );
      break;
    case OAuthStep.Completed:
      body = (
        <div className="notice notice-success" role="status">
          <strong>Account connected</strong>
          <p>New calls through this resource can use this account now.</p>
        </div>
      );
      break;
    case OAuthStep.Failed:
      body = (
        <div className="notice notice-error" role="alert">
          <strong>Connection failed</strong>
          <p>
            {flow?.failureMessage ||
              "The provider didn't approve the connection."}
          </p>
        </div>
      );
      break;
    case OAuthStep.Expired:
      body = (
        <div className="notice notice-error" role="alert">
          <strong>The request expired</strong>
          <p>Start again to get a new sign-in link.</p>
        </div>
      );
      break;
  }
  const footer =
    state.step === OAuthStep.Completed ? (
      <Button onClick={onClose}>Done</Button>
    ) : state.step === OAuthStep.Failed || state.step === OAuthStep.Expired ? (
      <>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
        <Button onClick={restart}>Start again</Button>
      </>
    ) : undefined;
  return (
    <Modal
      open
      onClose={onClose}
      title="Connect a provider account"
      description="Sign in with your provider account. Managed CPA keeps the account signed in for you."
      dismissible={!busy}
      footer={footer}
    >
      {body}
    </Modal>
  );
}

function CallbackUrlInput({
  value,
  onChange,
  ...rest
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  // The single critical input of this step receives focus when it appears.
  const ref = useFocusOnMount<HTMLInputElement>();
  return (
    <Input
      {...rest}
      ref={ref}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      placeholder="http://localhost:…/callback?code=…"
    />
  );
}

function ApiKeyDialog({
  resourceId,
  onClose,
}: {
  resourceId: string;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const [provider, setProvider] = useState(apiKeyProviders[0].provider);
  const [name, setName] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [models, setModels] = useState("");
  const [errors, setErrors] = useState<Record<string, string | undefined>>({});
  const [resubmit, setResubmit] = useState(false);
  const [failure, setFailure] = useState<unknown>();
  // The request carries the API key. Unmounting the dialog after success
  // removes the mutation from the query client; a failure discards it at once.
  const register = useSecretMutation(rpc.registerApiKeyCredential, {
    onSuccess: async () => {
      await invalidate(rpc.listCredentials);
      onClose();
    },
  });
  const compatible = provider === Provider.OPENAI_COMPATIBLE;
  const allowsBaseUrl = compatible || provider === Provider.VERTEX;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const modelList = parseList(models);
    const next = {
      name: nameError(name),
      apiKey: apiKey.trim() ? undefined : "Enter the API key.",
      baseUrl:
        compatible && !baseUrl.trim()
          ? "Enter the service's base URL."
          : allowsBaseUrl && baseUrl.trim() && !isHttpsUrl(baseUrl)
            ? "Enter a URL starting with https://."
            : undefined,
      models:
        compatible && !modelList.length
          ? "List at least one model name."
          : compatible
            ? modelList.map(modelNameError).find(Boolean)
            : undefined,
    };
    setErrors(next);
    if (Object.values(next).some(Boolean)) return;
    const secret = apiKey.trim();
    // The key is sent once and not kept in component state.
    setApiKey("");
    setResubmit(false);
    setFailure(undefined);
    register
      .mutateAsync({
        resourceId,
        provider,
        name: name.trim(),
        apiKey: secret,
        baseUrl: allowsBaseUrl ? baseUrl.trim() : "",
        models: compatible ? modelList : [],
      })
      .catch((error: unknown) => {
        // Keep only the error: the failed mutation's variables hold the key.
        register.discard();
        setFailure(error);
        setResubmit(true);
      });
  };
  return (
    <Modal
      open
      onClose={onClose}
      title="Add an API key"
      description="The key is encrypted and never shown again. Your provider bills you for its use."
      dismissible={!register.isPending}
    >
      <form className="form" onSubmit={submit} noValidate>
        <Field label="Provider">
          <NativeSelect
            autoFocus
            value={String(provider)}
            onChange={(event) =>
              setProvider(Number(event.target.value) as Provider)
            }
          >
            {apiKeyProviders.map((option) => (
              <option key={option.provider} value={String(option.provider)}>
                {option.label}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label="Name" error={errors.name}>
          <Input
            value={name}
            maxLength={100}
            onChange={(event) => setName(event.target.value)}
            placeholder="For example, Production OpenAI key"
          />
        </Field>
        <Field
          label="API key"
          error={errors.apiKey}
          hint={
            resubmit ? "For your security, enter the key again." : undefined
          }
        >
          <Input
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
          />
        </Field>
        {allowsBaseUrl && (
          <Field
            label={compatible ? "Base URL" : "Base URL (optional)"}
            error={errors.baseUrl}
            hint={
              compatible ? "For example, https://api.example.com/v1" : undefined
            }
          >
            <Input
              value={baseUrl}
              inputMode="url"
              onChange={(event) => setBaseUrl(event.target.value)}
            />
          </Field>
        )}
        {compatible && (
          <Field
            label="Models"
            error={errors.models}
            hint="Model names this service offers, separated by commas or new lines."
          >
            <Textarea
              value={models}
              onChange={(event) => setModels(event.target.value)}
            />
          </Field>
        )}
        {failure !== undefined && <ErrorState compact error={failure} />}
        <div className="form-actions">
          <Button type="submit" disabled={register.isPending}>
            {register.isPending ? "Adding…" : "Add API key"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RemoveCredentialDialog({
  resourceId,
  credential,
  onClose,
}: {
  resourceId: string;
  credential: Credential;
  onClose: () => void;
}) {
  const invalidate = useInvalidate();
  const remove = useMutation(rpc.removeCredential, {
    onSuccess: async () => {
      await invalidate(rpc.listCredentials);
      onClose();
    },
  });
  return (
    <Modal
      open
      onClose={onClose}
      title={`Remove ${credential.name || providerLabel(credential.provider)}?`}
      description="New calls stop using this credential right away. To use it again, connect or add it again."
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
              remove.mutate({ resourceId, credentialId: credential.id })
            }
          >
            {remove.isPending ? "Removing…" : "Remove credential"}
          </Button>
        </>
      }
    >
      {remove.error && <ErrorState compact error={remove.error} />}
    </Modal>
  );
}
