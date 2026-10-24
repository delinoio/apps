// SPDX-License-Identifier: Apache-2.0
import { useCallback, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { LogtoProvider, useHandleSignInCallback, useLogto } from "@logto/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { TransportProvider } from "@connectrpc/connect-query";
import { Code, ConnectError } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { queryClient } from "./core";
import { App } from "./app";
import { Button, Failure, Notice } from "./ui";
import "./style.css";
const delcoreURL =
  import.meta.env.PUBLIC_DELCORE_API_URL || "https://delcore.api.delino.io";
const delcoreResource =
  import.meta.env.PUBLIC_DELCORE_RESOURCE || "https://delcore.api.delino.io";
const perfineResource =
  import.meta.env.PUBLIC_PERFINE_RESOURCE || delcoreResource;
function Callback() {
  const { error } = useHandleSignInCallback(() => location.replace("/"));
  return (
    <main className="welcome">
      {error ? <Failure error={error} /> : <Notice title="Signing in…" />}
      <a href="/">Return to Perfine</a>
    </main>
  );
}
function Session() {
  const { isAuthenticated, isLoading, getAccessToken, signIn, signOut } =
    useLogto();
  const [client] = useState(queryClient);
  const token = useCallback(
    (resource: string) => getAccessToken(resource),
    [getAccessToken],
  );
  const transports = useMemo(() => {
    const make = (baseUrl: string, resource: string) =>
      createConnectTransport({
        baseUrl,
        interceptors: [
          (next) => async (req) => {
            const bearer = await token(resource);
            if (!bearer)
              throw new ConnectError(
                "Sign in to access this private space.",
                Code.Unauthenticated,
              );
            req.header.set("Authorization", `Bearer ${bearer}`);
            return next(req);
          },
        ],
      });
    return {
      perfine: make(
        import.meta.env.PUBLIC_PERFINE_API_URL ||
          "https://perfine.api.delino.io",
        perfineResource,
      ),
      delcore: make(delcoreURL, delcoreResource),
    };
  }, [token]);
  if (isLoading)
    return (
      <main className="welcome">
        <Notice title="Checking your session…" />
      </main>
    );
  if (!isAuthenticated) {
    client.clear();
    return (
      <main className="welcome">
        <div className="brand">◈ Perfine</div>
        <p className="eyebrow">PROFILE. COMPARE. DECIDE.</p>
        <h1>
          Performance claims,
          <br />
          backed by evidence.
        </h1>
        <p>
          Inspect experiments and review measured changes in your private space.
        </p>
        <Button onClick={() => void signIn(`${location.origin}/callback`)}>
          Sign in to your space
        </Button>
        <Notice title="Local first. Hosting is optional.">
          <p>
            Run <code>perfine init</code> and <code>perfine optimize</code>{" "}
            locally without a hosted account. No improvement and no candidate
            are valid outcomes.
          </p>
        </Notice>
      </main>
    );
  }
  return (
    <QueryClientProvider client={client}>
      <TransportProvider transport={transports.perfine}>
        <App
          delcore={transports.delcore}
          signOut={() => {
            client.clear();
            void signOut(location.origin);
          }}
        />
      </TransportProvider>
    </QueryClientProvider>
  );
}
const endpoint = import.meta.env.PUBLIC_LOGTO_ENDPOINT,
  appId = import.meta.env.PUBLIC_LOGTO_APP_ID;
createRoot(document.getElementById("root")!).render(
  endpoint && appId ? (
    <LogtoProvider
      config={{
        endpoint,
        appId,
        resources: [...new Set([delcoreResource, perfineResource])],
        scopes: ["openid", "profile", "email"],
      }}
    >
      {location.pathname === "/callback" ? <Callback /> : <Session />}
    </LogtoProvider>
  ) : (
    <main className="welcome">
      <div className="brand">◈ Perfine</div>
      <h1>Your performance workspace.</h1>
      <Notice title="Hosted sign-in is not configured">
        <p>
          The operator must configure the public Logto endpoint and application
          ID. Hosted results remain private.
        </p>
      </Notice>
      <p>Local optimization remains available without an account.</p>
      <code>
        perfine init
        <br />
        perfine optimize
      </code>
    </main>
  ),
);
