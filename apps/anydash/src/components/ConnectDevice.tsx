// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState, type ReactNode } from "@lynx-js/react";
import { createClient } from "@connectrpc/connect";
import {
  EnrollmentState,
  ErrorReason,
  LocalCollectorService,
  type LoginProgress,
} from "@repo/protos/anydash/v1/anydash_pb";
import { ExtraCopy } from "../copy";
import { toProblem } from "../errors";
import { hostCall, HostMethod } from "../host";
import { useSession } from "../session";
import { ConnectDeviceState } from "./States";
import { Modal } from "./dialog";
import { T } from "./primitives";

export const DESKTOP_DOWNLOAD_URL = "https://github.com/delinoio/apps/releases";
export const INSTALL_COMMANDS = [
  "curl -fsSL https://anydash.delino.io/install.sh | sh",
  "irm https://anydash.delino.io/install.ps1 | iex",
  "npm install -g @delinoio/anydash",
];

export interface CollectorLogin {
  /** Present only on the desktop app, which has a local collector. */
  start?: () => Promise<void>;
  /** The last failure, empty when none. */
  error: string;
  /** The approval dialog with the user code while an enrollment is pending. */
  dialog: ReactNode;
}

/**
 * Enrolls this computer through the desktop app's collector: starts the
 * enrollment, opens the approval URL in the browser, shows the user code and
 * polls the collector until the device is connected or the request ends.
 * Used by the personal space's onboarding and by Settings' This computer.
 */
export function useCollectorLogin(onConnected: () => void): CollectorLogin {
  const session = useSession();
  const [login, setLogin] = useState<LoginProgress>();
  const [error, setError] = useState("");
  const collector = session.collector
    ? createClient(LocalCollectorService, session.collector)
    : undefined;
  useEffect(() => {
    if (!login || !collector) return;
    const timer = setInterval(async () => {
      try {
        const { status } = await collector.getCollectorStatus({});
        const state = status?.login?.state;
        if (
          state === EnrollmentState.CONSUMED ||
          state === EnrollmentState.APPROVED ||
          (status?.deviceId && !status.login)
        ) {
          clearInterval(timer);
          setLogin(undefined);
          onConnected();
        } else if (
          state === EnrollmentState.EXPIRED ||
          state === EnrollmentState.INVALIDATED ||
          state === EnrollmentState.REJECTED
        ) {
          clearInterval(timer);
          setLogin(undefined);
          setError(ExtraCopy.connectFailed);
        }
      } catch {
        /* The next tick retries; the dialog stays open. */
      }
    }, 3000);
    return () => clearInterval(timer);
  }, [login?.userCode]);
  const start = async () => {
    if (!collector) return;
    setError("");
    try {
      const response = await collector.startLogin({ deviceName: "" });
      setLogin(response.login);
      if (response.login?.verificationUrl)
        await hostCall(HostMethod.OpenURL, {
          url: response.login.verificationUrl,
        });
    } catch (failure) {
      const problem = toProblem(failure);
      if (problem.reason === ErrorReason.ALREADY_LOGGED_IN) onConnected();
      else if (problem.reason === ErrorReason.UNSPECIFIED)
        setError(ExtraCopy.collectorUnavailable);
      else setError(problem.message);
    }
  };
  return {
    start: collector ? start : undefined,
    error,
    dialog: login ? (
      <Modal
        title={ExtraCopy.approveInBrowser}
        onClose={() => setLogin(undefined)}
      >
        <T muted>{ExtraCopy.approveInBrowserDetail}</T>
        <text className="ad-heading" style={{ textAlign: "center" }}>
          {login.userCode}
        </text>
      </Modal>
    ) : null,
  };
}

/** A collector login failure, shown under the action that started it. */
export function CollectorLoginError({ error }: { error: string }) {
  return error ? <T className="ad-text ad-danger-text">{error}</T> : null;
}

/**
 * The personal space's onboarding: web and mobile point to the desktop app and
 * the CLI; the desktop app enrolls this computer through its collector and
 * opens the same approval URL in the browser.
 */
export function ConnectDevice({ onConnected }: { onConnected: () => void }) {
  const [cli, setCli] = useState(false);
  const login = useCollectorLogin(onConnected);
  const start = login.start;
  return (
    <view className="lui-stack lui-column lui-gap-8">
      <ConnectDeviceState
        primaryLabel={start ? ExtraCopy.connectThisComputer : undefined}
        onPrimary={() =>
          start
            ? void start()
            : void hostCall(HostMethod.OpenURL, { url: DESKTOP_DOWNLOAD_URL })
        }
        onInstallCli={() => setCli(true)}
      />
      <CollectorLoginError error={login.error} />
      {cli ? (
        <Modal
          title={ExtraCopy.installCliTitle}
          onClose={() => setCli(false)}
          cancelLabel={ExtraCopy.close}
        >
          <T muted>{ExtraCopy.installCliDetail}</T>
          <view className="ad-ids">
            {INSTALL_COMMANDS.map((command) => (
              <text key={command} className="ad-mono">
                {command}
              </text>
            ))}
            <text className="ad-mono">anydash login</text>
          </view>
        </Modal>
      ) : null}
      {login.dialog}
    </view>
  );
}
