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
} from "@lynx-js/react";
import { createClient, type Transport } from "@connectrpc/connect";
import { TransportProvider } from "@connectrpc/connect-query";
import {
  MutationCache,
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from "@tanstack/react-query";
import {
  AccountService,
  DashboardService,
  LocalCollectorService,
  type Account,
  type Space,
} from "@repo/protos/anydash/v1/anydash_pb";
import { ProblemKind, shouldRetry, toProblem, type Problem } from "./errors";
import {
  clientKind,
  hostCall,
  HostEvent,
  HostMethod,
  HostPlatform,
  listenHost,
  RpcTarget,
  type HostInfo,
  type SessionState,
} from "./host";
import { logError } from "./log";
import { purgeUser } from "./model/cache";
import { createHostTransport } from "./transport";

export enum SessionPhase {
  Starting = "starting",
  SignedOut = "signed-out",
  SigningIn = "signing-in",
  Connecting = "connecting",
  Ready = "ready",
  Failed = "failed",
}

export interface SessionValue {
  phase: SessionPhase;
  info: HostInfo;
  account?: Account;
  spaces: Space[];
  userKey: string;
  problem?: Problem;
  updateRequired: boolean;
  transport: Transport;
  /** LocalCollectorService transport; present on desktop hosts. */
  collector?: Transport;
  signIn(): Promise<void>;
  signOut(): Promise<void>;
  retry(): void;
  reloadSpaces(): Promise<Space[]>;
  setAccount(account: Account): void;
  reportProblem(error: unknown): Problem;
}

const fallbackInfo: HostInfo = {
  platform: HostPlatform.Web,
  collector: false,
  platformLabel: "Web",
  hostVersion: "",
};
const Session = createContext<SessionValue | null>(null);
export function useSession() {
  const value = useContext(Session);
  if (!value) throw new Error("SessionProvider is missing.");
  return value;
}

export function createQueryClient(onProblem: (error: unknown) => void) {
  return new QueryClient({
    queryCache: new QueryCache({ onError: onProblem }),
    mutationCache: new MutationCache({ onError: onProblem }),
    defaultOptions: {
      queries: {
        retry: shouldRetry,
        staleTime: 30_000,
        refetchOnWindowFocus: false,
      },
      mutations: { retry: false },
    },
  });
}

export function SessionProvider({
  children,
  info: initialInfo,
  queryClient: providedClient,
}: {
  children: ReactNode;
  info?: HostInfo;
  queryClient?: QueryClient;
}) {
  const [info, setInfo] = useState<HostInfo>(initialInfo ?? fallbackInfo);
  const [phase, setPhase] = useState(SessionPhase.Starting);
  const [account, setAccount] = useState<Account>();
  const [spaces, setSpaces] = useState<Space[]>([]);
  const [userKey, setUserKey] = useState("");
  const [problem, setProblem] = useState<Problem>();
  const [updateRequired, setUpdateRequired] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const kind = useRef(clientKind(info.platform));
  kind.current = clientKind(info.platform);
  const transport = useMemo(
    () => createHostTransport(RpcTarget.Api, () => kind.current),
    [],
  );
  const collector = useMemo(
    () =>
      info.collector
        ? createHostTransport(RpcTarget.Collector, () => kind.current)
        : undefined,
    [info.collector],
  );
  const reportProblem = useCallback((error: unknown) => {
    const next = toProblem(error);
    if (next.kind === ProblemKind.UpdateRequired) setUpdateRequired(true);
    if (next.kind === ProblemKind.SignedOut) setPhase(SessionPhase.SignedOut);
    return next;
  }, []);
  const queryClient = useMemo(
    () => providedClient ?? createQueryClient((error) => reportProblem(error)),
    [],
  );
  const accounts = useMemo(() => createClient(AccountService, transport), []);
  const dashboards = useMemo(
    () => createClient(DashboardService, transport),
    [],
  );

  const reloadSpaces = useCallback(async () => {
    // The host sets delcore_access_token; the token never enters the bundle.
    const response = await dashboards.listSpaces({});
    setSpaces(response.spaces);
    return response.spaces;
  }, []);

  useEffect(() => {
    let active = true;
    (async () => {
      try {
        const host = initialInfo ?? (await hostCall<HostInfo>(HostMethod.Info));
        if (!active) return;
        setInfo(host);
        const session = await hostCall<SessionState>(HostMethod.Session);
        if (!active) return;
        if (!session.signedIn) {
          setPhase(SessionPhase.SignedOut);
          return;
        }
        setUserKey(session.userKey ?? "");
        setPhase(SessionPhase.Connecting);
        const connected = await accounts.connectAccount({});
        if (!active) return;
        setAccount(connected.account);
        await reloadSpaces();
        if (!active) return;
        setProblem(undefined);
        setPhase(SessionPhase.Ready);
      } catch (error) {
        if (!active) return;
        const next = reportProblem(error);
        logError("anydash_session_failed", {
          kind: next.kind,
          correlationId: next.correlationId,
        });
        setProblem(next);
        setPhase(
          next.kind === ProblemKind.SignedOut
            ? SessionPhase.SignedOut
            : SessionPhase.Failed,
        );
      }
    })();
    return () => {
      active = false;
    };
  }, [attempt]);

  useEffect(
    () =>
      listenHost(HostEvent.Session, () => {
        queryClient.clear();
        setAttempt((value) => value + 1);
      }),
    [],
  );

  const value: SessionValue = {
    phase,
    info,
    account,
    spaces,
    userKey,
    problem,
    updateRequired,
    transport,
    collector,
    reportProblem,
    setAccount,
    reloadSpaces,
    retry: () => setAttempt((value) => value + 1),
    async signIn() {
      setPhase(SessionPhase.SigningIn);
      try {
        await hostCall(HostMethod.SignIn);
        setAttempt((value) => value + 1);
      } catch (error) {
        setProblem(toProblem(error));
        setPhase(SessionPhase.SignedOut);
      }
    },
    async signOut() {
      // A collector device also deletes its device credentials and stops
      // collection; the server keeps the registration for reattachment.
      if (collector) {
        try {
          await createClient(LocalCollectorService, collector).logout({});
        } catch (error) {
          const next = toProblem(error);
          if (next.kind !== ProblemKind.Unknown)
            logError("anydash_collector_logout_failed", { kind: next.kind });
        }
      }
      await purgeUser(userKey);
      queryClient.clear();
      await hostCall(HostMethod.SignOut).catch(() => undefined);
      setAccount(undefined);
      setSpaces([]);
      setUserKey("");
      setPhase(SessionPhase.SignedOut);
    },
  };
  return (
    <Session.Provider value={value}>
      <QueryClientProvider client={queryClient}>
        <TransportProvider transport={transport}>{children}</TransportProvider>
      </QueryClientProvider>
    </Session.Provider>
  );
}
