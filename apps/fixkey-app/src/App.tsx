import { Button } from "@delino/ui/components/button";
import { Card } from "@delino/ui/components/card";
import { Checkbox } from "@delino/ui/components/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@delino/ui/components/dialog";
import { Input } from "@delino/ui/components/input";
import { Label } from "@delino/ui/components/label";
import { NativeSelect } from "@delino/ui/components/native-select";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  desktopApi,
  errorCode,
  type CandidateState,
  type DesktopApi,
  type ExcludedApp,
  type Settings,
} from "./api";
import { messages } from "./locale";

enum Section {
  General = "general",
  Exclusions = "exclusions",
  Shortcuts = "shortcuts",
  Updates = "updates",
  Diagnostics = "diagnostics",
}

function Modal({
  title,
  description,
  children,
  onClose,
}: {
  title: string;
  description: string;
  children: ReactNode;
  onClose(): void;
}) {
  const previousFocus = useRef(document.activeElement);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="settings-dialog"
        showCloseButton={false}
        onPointerDownOutside={(event) => event.preventDefault()}
        onOpenAutoFocus={(event) => {
          const target = event.currentTarget as HTMLElement;
          const control = target.querySelector<HTMLElement>(
            "[data-autofocus], [autofocus]",
          );
          if (control && !control.matches(":disabled")) {
            event.preventDefault();
            control.focus();
          }
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          if (
            previousFocus.current instanceof HTMLElement &&
            previousFocus.current.isConnected
          )
            previousFocus.current.focus();
        }}
      >
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
        {children}
      </DialogContent>
    </Dialog>
  );
}

export function App({ api = desktopApi }: { api?: DesktopApi }) {
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["desktop"],
    queryFn: () => api.state(),
    refetchInterval: 1500,
    retry: false,
  });
  const [section, setSection] = useState(Section.General);
  const [error, setError] = useState<string | null>(null);
  const [permissionDismissed, setPermissionDismissed] = useState(false);
  const [inputSetupOpen, setInputSetupOpen] = useState(false);
  const permissionRequested = useRef(false);
  const [capture, setCapture] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [apps, setApps] = useState<ExcludedApp[]>([]);
  const [appIndex, setAppIndex] = useState(0);
  const data = query.data;
  const language =
    data?.settings.uiLanguage === "system"
      ? data.osLanguage
      : (data?.settings.uiLanguage ?? "en");
  const t = messages[language];
  const save = useMutation({
    mutationFn: (settings: Settings) => api.save(settings, data!.revision),
    onSuccess: (state) => {
      client.setQueryData(["desktop"], state);
      setError(null);
    },
    onError: (reason) => {
      setError(errorCode(reason));
      void query.refetch();
    },
  });
  const action = useMutation({
    mutationFn: (name: Parameters<DesktopApi["action"]>[0]) => api.action(name),
    onSuccess: (_, name) => {
      if (name === "recheck") setInputSetupOpen(false);
      setError(null);
      void query.refetch();
    },
    onError: (reason) => setError(errorCode(reason)),
  });
  const requestPermission = action.mutate;
  useEffect(() => {
    if (
      data?.platform === "macos-arm64" &&
      (data.status === "permission-missing" ||
        data.status === "input-permission-missing") &&
      !permissionDismissed &&
      !permissionRequested.current
    ) {
      // Request from FixKey itself so macOS can list this app in Accessibility.
      // Polling must never repeat a dismissed system prompt.
      permissionRequested.current = true;
      requestPermission("request-permission");
    }
  }, [data?.platform, data?.status, permissionDismissed, requestPermission]);
  useEffect(() => {
    document.documentElement.lang = language;
    document.documentElement.dataset.theme = data?.settings.theme ?? "system";
  }, [language, data?.settings.theme]);
  const busy = save.isPending || action.isPending;
  const change = (patch: Partial<Settings>) => {
    if (data && !busy) save.mutate({ ...data.settings, ...patch });
  };
  const chooseApp = async () => {
    try {
      setApps(await api.apps());
      setAppIndex(0);
      setChoosing(true);
      setError(null);
    } catch (reason) {
      setError(errorCode(reason));
    }
  };
  if (!data)
    return (
      <main className="connection">
        <h1>FixKey</h1>
        <p role="status">{query.isError ? t.connection : t.loading}</p>
        {query.isError && (
          <Button variant="outline" onClick={() => void query.refetch()}>
            {t.retry}
          </Button>
        )}
      </main>
    );
  const settings = data.settings;
  const waitingForField = !data.active && data.status === "unknown-field";
  const permissionNeeded =
    data.status === "permission-missing" ||
    data.status === "input-permission-missing";
  const inputPermissionNeeded =
    data.platform === "macos-arm64" &&
    (data.status === "input-permission-missing" || inputSetupOpen);
  const closePermission = () => {
    setInputSetupOpen(false);
    setPermissionDismissed(true);
  };
  const permissionTitle = inputSetupOpen
    ? t.inputPermissionSetup
    : inputPermissionNeeded
      ? t.inputPermission
      : t.permission;
  const permissionRequestLabel = inputPermissionNeeded
    ? t.inputPermissionRequest
    : t.permissionRequest;
  const permissionInstruction =
    data.platform === "macos-arm64"
      ? t.macPermission
      : data.platform === "windows-x64" || data.platform === "windows-arm64"
        ? t.windowsPermission
        : t.linuxPermission;
  const row = (label: string, control: ReactNode) => (
    <div className="row">
      <span>{label}</span>
      {control}
    </div>
  );
  const toggle = (label: string, checked: boolean, field: keyof Settings) => (
    <Label className="row" htmlFor={field}>
      <span>{label}</span>
      <Checkbox
        id={field}
        checked={checked}
        disabled={busy}
        onCheckedChange={(checked) => change({ [field]: checked === true })}
      />
    </Label>
  );
  const select = (
    label: string,
    value: string,
    field: "theme" | "uiLanguage",
    options: [string, string][],
  ) => (
    <Label className="row">
      <span>{label}</span>
      <NativeSelect
        aria-label={label}
        value={value}
        disabled={busy}
        onChange={(e) => change({ [field]: e.target.value })}
      >
        {options.map(([key, name]) => (
          <option value={key} key={key}>
            {name}
          </option>
        ))}
      </NativeSelect>
    </Label>
  );
  return (
    <div className="shell">
      <aside>
        <h1>FixKey</h1>
        <p className="tagline">{t.tagline}</p>
        <nav aria-label="FixKey">
          {Object.values(Section).map((key) => (
            <Button
              variant="outline"
              key={key}
              aria-current={section === key ? "page" : undefined}
              onClick={() => setSection(key)}
            >
              {t[key]}
            </Button>
          ))}
        </nav>
        <p className="version">v{data.version}</p>
      </aside>
      <main>
        <h2>{t[section]}</h2>
        <div className={`status ${data.active ? "active" : ""}`} role="status">
          <strong>
            {data.active
              ? t.active
              : waitingForField
                ? t.waitingField
                : t.inactive}
          </strong>
          {waitingForField ? (
            <span>{t.waitingFieldHelp}</span>
          ) : (
            <span>
              {t.reason}: <code>{data.status}</code>
            </span>
          )}
        </div>
        {(error || data.settingsError || !data.logsHealthy) && (
          <div role="alert" className="error">
            <strong>{t.error}</strong>
            <p>
              {t.reason}:{" "}
              <code>{error ?? data.settingsError ?? "log-write-failed"}</code>
            </p>
            {data.settingsError && (
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => action.mutate("restore-backup")}
              >
                {t.restore}
              </Button>
            )}
          </div>
        )}
        {data.diagnosticError && (
          <p role="alert">
            {t.reason}: <code>{data.diagnosticError}</code>
          </p>
        )}
        <fieldset disabled={busy} className="controls">
          {section === Section.General && (
            <>
              <Card asChild>
                <section className="card">
                  {row(
                    t.correction,
                    <Button
                      variant="default"
                      className="primary"
                      onClick={() => change({ paused: !settings.paused })}
                    >
                      {settings.paused ? t.resume : t.pause}
                    </Button>,
                  )}
                  <p>{t.local}</p>
                  <Button
                    variant="outline"
                    onClick={() => {
                      setPermissionDismissed(false);
                      action.mutate("recheck");
                    }}
                  >
                    {t.recheck}
                  </Button>
                  {data.platform === "macos-arm64" && (
                    <Button
                      variant="outline"
                      onClick={() => {
                        setInputSetupOpen(true);
                        setPermissionDismissed(false);
                      }}
                    >
                      {t.inputPermissionSetup}
                    </Button>
                  )}
                </section>
              </Card>
              <Card asChild>
                <section className="card">
                  <h3>{t.languages}</h3>
                  {toggle(t.korean, settings.koreanEnabled, "koreanEnabled")}
                  {toggle(t.english, settings.englishEnabled, "englishEnabled")}
                </section>
              </Card>
              <Card asChild>
                <section className="card">
                  {toggle(t.startup, settings.loginStartup, "loginStartup")}
                  {select(t.uiLanguage, settings.uiLanguage, "uiLanguage", [
                    ["system", t.system],
                    ["en", "English"],
                    ["ko", "한국어"],
                  ])}
                  {select(t.theme, settings.theme, "theme", [
                    ["system", t.system],
                    ["light", t.light],
                    ["dark", t.dark],
                  ])}
                </section>
              </Card>
            </>
          )}
          {section === Section.Exclusions && (
            <Card asChild>
              <section className="card">
                <div className="row">
                  <h3>{t.exclusions}</h3>
                  <Button variant="outline" onClick={() => void chooseApp()}>
                    {t.add}
                  </Button>
                </div>
                {settings.excludedApps.length === 0 ? (
                  <div className="empty">
                    <h3>{t.empty}</h3>
                    <p>{t.emptyDetail}</p>
                  </div>
                ) : (
                  <ul className="apps">
                    {settings.excludedApps.map((app, index) => (
                      <li key={JSON.stringify(app.identity)}>
                        <div>
                          <strong>{app.displayName}</strong>
                          {data.unresolvedApps.includes(index) && (
                            <p>{t.unresolved}</p>
                          )}
                        </div>
                        <Button
                          variant="outline"
                          aria-label={`${t.remove} ${app.displayName}`}
                          onClick={() =>
                            change({
                              excludedApps: settings.excludedApps.filter(
                                (_, i) => i !== index,
                              ),
                            })
                          }
                        >
                          {t.remove}
                        </Button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            </Card>
          )}
          {section === Section.Shortcuts && (
            <Card asChild>
              <section className="card">
                {row(
                  t.shortcut,
                  <kbd>{settings.shortcut ?? t.unassigned}</kbd>,
                )}
                <div className="actions">
                  <Button variant="outline" onClick={() => setCapture(true)}>
                    {t.capture}
                  </Button>
                  <Button
                    variant="outline"
                    disabled={!settings.shortcut}
                    onClick={() => change({ shortcut: null })}
                  >
                    {t.clear}
                  </Button>
                </div>
              </section>
            </Card>
          )}
          {section === Section.Updates && (
            <Card asChild>
              <section className="card">
                {row(t.currentVersion, <span>{data.version}</span>)}
                <p>{t.updateHelp}</p>
                <p role="status">
                  <code>{data.update.phase}</code>
                  {data.update.version && ` · ${data.update.version}`}
                </p>
                {data.update.error && (
                  <p role="alert">
                    {t.reason}: <code>{data.update.error}</code>
                  </p>
                )}
                {data.update.phase === "downloading" && (
                  <>
                    <progress
                      aria-label={t.updateConsent}
                      max={100}
                      value={data.update.progress ?? undefined}
                    />
                    <span>
                      {data.update.progress == null
                        ? "…"
                        : `${Math.round(data.update.progress)}%`}
                    </span>
                  </>
                )}
                <div className="actions">
                  <Button
                    variant="outline"
                    disabled={[
                      "checking",
                      "downloading",
                      "installing",
                    ].includes(data.update.phase)}
                    onClick={() => action.mutate("check-update")}
                  >
                    {t.checkUpdate}
                  </Button>
                  {data.update.phase === "available" && (
                    <Button
                      variant="default"
                      className="primary"
                      onClick={() => action.mutate("install-update")}
                    >
                      {t.updateConsent}
                    </Button>
                  )}
                </div>
                <p>{t.recovery}</p>
              </section>
            </Card>
          )}
          {section === Section.Diagnostics && (
            <>
              <Card asChild>
                <section className="card">
                  {row(t.reason, <code>{data.status}</code>)}
                  <p>{t.logHelp}</p>
                  <div className="actions">
                    <Button
                      variant="outline"
                      onClick={() => action.mutate("open-logs")}
                    >
                      {t.openLogs}
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => action.mutate("delete-logs")}
                    >
                      {t.deleteLogs}
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => action.mutate("support")}
                    >
                      {t.support}
                    </Button>
                  </div>
                </section>
              </Card>
              <Card asChild>
                <section className="card">
                  <h3>{t.help}</h3>
                  {t.helpTopics.map(([title, body]) => (
                    <details key={title}>
                      <summary>{title}</summary>
                      <p>{body}</p>
                    </details>
                  ))}
                </section>
              </Card>
            </>
          )}
        </fieldset>
      </main>
      {(permissionNeeded || inputSetupOpen) && !permissionDismissed && (
        <Modal
          title={permissionTitle}
          description={
            inputSetupOpen
              ? t.inputPermissionListHelp
              : inputPermissionNeeded
                ? t.inputPermissionHelp
                : t.permissionHelp
          }
          onClose={closePermission}
        >
          {data.status === "input-permission-missing" && (
            <p className="permission-granted">{t.accessibilityGranted}</p>
          )}
          <p className="permission-privacy">{t.local}</p>
          {data.platform === "macos-arm64" ? (
            <ol className="permission-steps">
              {(inputPermissionNeeded
                ? t.inputPermissionSteps
                : t.permissionSteps
              ).map((step) => (
                <li key={step}>{step}</li>
              ))}
            </ol>
          ) : (
            <p>{permissionInstruction}</p>
          )}
          {!inputSetupOpen && <p role="status">{t.permissionWaiting}</p>}
          {error && (
            <p role="alert">
              {t.error}: <code>{error}</code>
            </p>
          )}
          <div className="actions">
            <Button
              variant="default"
              className="primary"
              data-autofocus
              disabled={busy}
              onClick={() => action.mutate("request-permission")}
            >
              {data.platform === "macos-arm64"
                ? permissionRequestLabel
                : t.openSettings}
            </Button>
            {data.platform === "macos-arm64" && (
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => action.mutate("permission-settings")}
              >
                {t.openSettings}
              </Button>
            )}
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => action.mutate("recheck")}
            >
              {t.recheck}
            </Button>
            <Button variant="outline" onClick={closePermission}>
              {t.later}
            </Button>
          </div>
        </Modal>
      )}
      {capture && (
        <Modal
          title={t.capture}
          description={t.captureHelp}
          onClose={() => setCapture(false)}
        >
          <Input
            autoFocus
            readOnly
            aria-label={t.capture}
            placeholder={t.unassigned}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setCapture(false);
                return;
              }
              e.preventDefault();
              if (
                ["Control", "Alt", "Shift", "Meta"].includes(e.key) ||
                !(e.ctrlKey || e.altKey || e.metaKey || e.shiftKey)
              )
                return;
              const key = e.code.startsWith("Key")
                ? e.code.slice(3)
                : e.code.startsWith("Digit")
                  ? e.code.slice(5)
                  : e.code;
              change({
                shortcut: [
                  e.ctrlKey && "Control",
                  e.altKey && "Alt",
                  e.shiftKey && "Shift",
                  e.metaKey && "Super",
                  key,
                ]
                  .filter(Boolean)
                  .join("+"),
              });
              setCapture(false);
            }}
          />
          <Button variant="outline" onClick={() => setCapture(false)}>
            {t.cancel}
          </Button>
        </Modal>
      )}
      {choosing && (
        <Modal
          title={t.choose}
          description={t.exclusions}
          onClose={() => setChoosing(false)}
        >
          {apps.length ? (
            <>
              <NativeSelect
                autoFocus
                aria-label={t.choose}
                value={appIndex}
                onChange={(e) => setAppIndex(Number(e.target.value))}
              >
                {apps.map((app, i) => (
                  <option key={JSON.stringify(app.identity)} value={i}>
                    {app.displayName}
                  </option>
                ))}
              </NativeSelect>
              <Button
                variant="default"
                className="primary"
                onClick={() => {
                  const app = apps[appIndex];
                  if (
                    !settings.excludedApps.some(
                      (other) =>
                        JSON.stringify(other.identity) ===
                        JSON.stringify(app.identity),
                    )
                  )
                    change({ excludedApps: [...settings.excludedApps, app] });
                  setChoosing(false);
                }}
              >
                {t.add}
              </Button>
            </>
          ) : (
            <p>{t.noApps}</p>
          )}
          <Button variant="outline" onClick={() => setChoosing(false)}>
            {t.cancel}
          </Button>
        </Modal>
      )}
    </div>
  );
}

export function CandidatePicker({
  api = desktopApi,
  language = "en",
}: {
  api?: DesktopApi;
  language?: "en" | "ko";
}) {
  const [state, setState] = useState<CandidateState>({
    generation: 0,
    selected: 0,
    items: [],
  });
  const [error, setError] = useState<string | null>(null);
  const t = messages[state.language ?? language];
  useEffect(() => {
    document.documentElement.dataset.theme = state.theme ?? "system";
    document.documentElement.lang = state.language ?? language;
  }, [state.theme, state.language, language]);
  useEffect(() => {
    let cancelled = false;
    let stop: (() => void) | undefined;
    void api
      .candidates((next) => {
        if (!cancelled)
          setState((old) =>
            next.generation > old.generation ||
            (next.generation === old.generation &&
              (next.revision ?? 0) >= (old.revision ?? 0))
              ? next
              : old,
          );
      })
      .then((unsubscribe) => {
        if (cancelled) unsubscribe();
        else stop = unsubscribe;
      })
      .catch(() => setError("candidate-unavailable"));
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [api]);
  const act = (
    action: "next" | "previous" | "apply" | "close",
    index?: number,
  ) => {
    if (action === "close") setState({ ...state, items: [] });
    void api
      .candidateAction(state.generation, action, index)
      .catch((reason) => {
        setError(errorCode(reason));
        setState((old) => ({ ...old, items: [] }));
      });
  };
  return (
    <main
      className="picker"
      onKeyDown={(e) => {
        const key = {
          ArrowDown: "next",
          ArrowUp: "previous",
          Enter: "apply",
          Escape: "close",
        }[e.key] as "next" | "previous" | "apply" | "close" | undefined;
        if (key) {
          e.preventDefault();
          act(key);
        }
      }}
    >
      <h1>{t.candidates}</h1>
      <p>{t.candidateHelp}</p>
      {state.items.length ? (
        <div role="listbox" aria-label={t.candidates}>
          {state.items.map((item, i) => (
            <Button
              variant="outline"
              key={`${item.language}-${i}`}
              role="option"
              aria-selected={state.selected === i}
              onClick={() => act("apply", i)}
            >
              <strong>{item.text}</strong>
              <small>
                {item.isOriginal
                  ? `${t.original} · `
                  : `${item.language === "ko-dubeolsik" ? t.korean : t.english} · `}
                {item.validation === "recognized"
                  ? t.recognized
                  : item.validation === "not-recognized"
                    ? t.notRecognized
                    : t.unchecked}
              </small>
            </Button>
          ))}
        </div>
      ) : (
        <p role="status">{t.noCandidates}</p>
      )}
      {error && (
        <p role="alert">
          {t.reason}: {error}
        </p>
      )}
    </main>
  );
}
