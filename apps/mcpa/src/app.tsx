// SPDX-License-Identifier: Apache-2.0
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  MutationCache,
  QueryCache,
  QueryClient,
  QueryClientProvider,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { TransportProvider } from "@connectrpc/connect-query";
import type { Transport } from "@connectrpc/connect";
import { Button } from "@packages/ui/components/button";
import { describeError, ErrorKind } from "./lib/errors";
import { RouteName, RouterProvider, useRouter } from "./lib/router";
import {
  consoleReturnUrl,
  rememberReturnPath,
  SessionStatus,
  takeReturnPath,
  type Session,
  type SessionClient,
} from "./lib/session";
import { createMcpaTransport } from "./lib/transport";
import { Loading } from "./components/states";
import { ConsoleShell } from "./screens/shell";

export const SESSION_QUERY_KEY = ["mcpa", "auth-session"] as const;

type SignedInSession = Extract<Session, { status: SessionStatus.SignedIn }>;
type SessionContextValue = {
  session: SignedInSession;
  signOut: () => Promise<void>;
};
const SessionContext = createContext<SessionContextValue | undefined>(
  undefined,
);

export function useSession(): SessionContextValue {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession must be used inside a session.");
  return value;
}

export type AppProps = {
  apiUrl: string;
  sessionClient: SessionClient;
  /** Overrides the management transport, for example in tests. */
  transport?: Transport;
  queryClient?: QueryClient;
};

/**
 * Creates the console query client. A request that still fails
 * authentication after the transport's refresh retry ends the session: MFA
 * failures show the MFA screen and other failures show sign-in.
 */
export function createQueryClient(): QueryClient {
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({ onError: (error) => handle(error) }),
    mutationCache: new MutationCache({ onError: (error) => handle(error) }),
    defaultOptions: {
      queries: { retry: 1, staleTime: 15_000, refetchOnWindowFocus: true },
    },
  });
  const handle = (error: unknown) => {
    const description = describeError(error);
    if (description.kind === ErrorKind.MfaRequired) {
      const next: Session = {
        status: SessionStatus.MfaRequired,
        reauthenticationUrl: description.reauthenticationUrl ?? "",
      };
      client.setQueryData<Session>(SESSION_QUERY_KEY, next);
    } else if (description.kind === ErrorKind.Unauthenticated)
      void client.invalidateQueries({ queryKey: SESSION_QUERY_KEY });
  };
  return client;
}

export function App(props: AppProps) {
  const [queryClient] = useState(
    () => props.queryClient ?? createQueryClient(),
  );
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider>
        <SessionGate {...props} />
      </RouterProvider>
    </QueryClientProvider>
  );
}

function SessionGate({ apiUrl, sessionClient, transport }: AppProps) {
  const queryClient = useQueryClient();
  const { route, navigate } = useRouter();
  const sessionQuery = useQuery({
    queryKey: SESSION_QUERY_KEY,
    queryFn: () => sessionClient.getSession(),
    retry: 1,
    staleTime: 60_000,
  });
  const session = sessionQuery.data;
  const csrf = useRef<string | undefined>(undefined);
  csrf.current =
    session?.status === SessionStatus.SignedIn ? session.csrfToken : undefined;

  // Refreshes the session and reloads it. Concurrent callers share one
  // refresh so a burst of failed requests sends a single POST /auth/refresh.
  const recovering = useRef<Promise<boolean> | undefined>(undefined);
  const recover = useCallback(() => {
    recovering.current ??= (async () => {
      const token = csrf.current;
      if (!token) return false;
      await sessionClient.refresh(token).catch(() => undefined);
      try {
        const next = await sessionClient.getSession();
        queryClient.setQueryData<Session>(SESSION_QUERY_KEY, next);
        return next.status === SessionStatus.SignedIn;
      } catch {
        return false;
      }
    })().finally(() => {
      recovering.current = undefined;
    });
    return recovering.current;
  }, [sessionClient, queryClient]);

  const managementTransport = useMemo(
    () =>
      transport ??
      createMcpaTransport(apiUrl, { csrfToken: () => csrf.current, recover }),
    [apiUrl, transport, recover],
  );

  // After sign-in the API returns to the console root; restore the page the
  // user was on when they started signing in.
  const signedIn = session?.status === SessionStatus.SignedIn;
  useEffect(() => {
    if (!signedIn) return;
    const path = takeReturnPath();
    if (path !== "/" && route.name === RouteName.Home)
      navigate(path, { replace: true });
    // Runs once per sign-in; later navigation must not be overridden.
  }, [signedIn]);

  // Refresh the session shortly before it expires.
  const expiresAt =
    session?.status === SessionStatus.SignedIn ? session.expiresAt : undefined;
  useEffect(() => {
    if (!expiresAt) return;
    const delay = Math.max(expiresAt.getTime() - Date.now() - 60_000, 5_000);
    const timer = setTimeout(() => void recover(), delay);
    return () => clearTimeout(timer);
  }, [expiresAt, recover]);

  const value = useMemo<SessionContextValue | undefined>(() => {
    if (session?.status !== SessionStatus.SignedIn) return undefined;
    return {
      session,
      signOut: async () => {
        await sessionClient.logout(session.csrfToken);
        queryClient.clear();
        queryClient.setQueryData<Session>(SESSION_QUERY_KEY, {
          status: SessionStatus.SignedOut,
        });
      },
    };
  }, [session, sessionClient, queryClient]);

  if (sessionQuery.isPending)
    return (
      <CenteredPage>
        <Loading label="Checking your session…" />
      </CenteredPage>
    );
  if (!session)
    return (
      <CenteredPage>
        <div className="notice notice-error" role="alert">
          <strong>We couldn't check your session</strong>
          <p>
            Managed CPA didn't respond. Check your connection and try again.
          </p>
          <Button variant="outline" onClick={() => void sessionQuery.refetch()}>
            Try again
          </Button>
        </div>
      </CenteredPage>
    );
  const loginUrl = sessionClient.loginUrl(consoleReturnUrl());
  if (session.status === SessionStatus.SignedOut)
    return <SignInPage loginUrl={loginUrl} />;
  if (session.status === SessionStatus.MfaRequired)
    return (
      <MfaRequiredPage
        reauthenticationUrl={session.reauthenticationUrl}
        loginUrl={loginUrl}
        onCheckAgain={() => void sessionQuery.refetch()}
        checking={sessionQuery.isFetching}
      />
    );
  return (
    <SessionContext.Provider value={value}>
      <TransportProvider transport={managementTransport}>
        <ConsoleShell />
      </TransportProvider>
    </SessionContext.Provider>
  );
}

function CenteredPage({ children }: { children: ReactNode }) {
  return (
    <main className="centered-page">
      <div className="centered-card">{children}</div>
    </main>
  );
}

function rememberCurrentPage() {
  const { pathname, search } = window.location;
  rememberReturnPath(`${pathname}${search}`);
}

export function SignInPage({ loginUrl }: { loginUrl: string }) {
  return (
    <CenteredPage>
      <p className="eyebrow">Managed CPA</p>
      <h1>Sign in to continue</h1>
      <p className="lead">
        Use your Delino account. Managed CPA requires two-step verification.
      </p>
      <Button asChild size="lg" className="full">
        <a href={loginUrl} onClick={rememberCurrentPage}>
          Sign in
        </a>
      </Button>
    </CenteredPage>
  );
}

export function MfaRequiredPage({
  reauthenticationUrl,
  loginUrl,
  onCheckAgain,
  checking,
}: {
  reauthenticationUrl: string;
  loginUrl: string;
  onCheckAgain: () => void;
  checking: boolean;
}) {
  return (
    <CenteredPage>
      <p className="eyebrow">Managed CPA</p>
      <h1>Turn on two-step verification</h1>
      <p className="lead">
        Managed CPA protects provider credentials and call keys, so it accepts
        only sign-ins that used two-step verification. Set it up for your Delino
        account, or sign in again with it.
      </p>
      <Button asChild size="lg" className="full">
        <a href={reauthenticationUrl || loginUrl} onClick={rememberCurrentPage}>
          {reauthenticationUrl
            ? "Set up two-step verification"
            : "Sign in again"}
        </a>
      </Button>
      <Button
        variant="ghost"
        className="full"
        disabled={checking}
        onClick={onCheckAgain}
      >
        {checking ? "Checking…" : "I've done this. Check again"}
      </Button>
    </CenteredPage>
  );
}
