// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "@lynx-js/react";
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

/**
 * The personal space's onboarding: web and mobile point to the desktop app and
 * the CLI; the desktop app enrolls this computer through its collector and
 * opens the same approval URL in the browser.
 */
export function ConnectDevice({ onConnected }: { onConnected: () => void }) {
  const session = useSession();
  const [cli, setCli] = useState(false);
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
  const startDesktop = async () => {
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
  return (
    <view className="lui-stack lui-column lui-gap-8">
      <ConnectDeviceState
        primaryLabel={collector ? ExtraCopy.connectThisComputer : undefined}
        onPrimary={() =>
          collector
            ? void startDesktop()
            : void hostCall(HostMethod.OpenURL, { url: DESKTOP_DOWNLOAD_URL })
        }
        onInstallCli={() => setCli(true)}
      />
      {error ? <T className="ad-text ad-danger-text">{error}</T> : null}
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
      {login ? (
        <Modal
          title={ExtraCopy.approveInBrowser}
          onClose={() => setLogin(undefined)}
        >
          <T muted>{ExtraCopy.approveInBrowserDetail}</T>
          <text className="ad-heading" style={{ textAlign: "center" }}>
            {login.userCode}
          </text>
        </Modal>
      ) : null}
    </view>
  );
}
