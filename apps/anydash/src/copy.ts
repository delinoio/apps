// SPDX-License-Identifier: Apache-2.0
// Final English copy approved in the AnyDash design record. Change these
// strings only with a new design approval; tests assert them verbatim.

export const Copy = {
  appName: "AnyDash",
  refresh: "Refresh",
  edit: "Edit",
  done: "Done",
  cancel: "Cancel",
  loading: "Loading…",
  loadingDashboard: "Loading dashboard…",
  headerUpdated: (timeZone: string, time: string) =>
    `${timeZone} · Updated ${time}`,

  notProvided: "Not provided",
  notProvidedDetail: (provider: string) =>
    `${provider} doesn’t report this value.`,
  couldNotRefresh: "Couldn’t refresh",
  couldNotRefreshDetail: (time: string, minutes: number) =>
    `Last updated ${time} · Retrying in ${minutes} min`,
  couldNotRefreshSince: (time: string) => `Last updated ${time}`,
  refreshAvailableIn: (seconds: number) => `Refresh available in ${seconds}s`,
  providerAskedToWait: (provider: string, time: string) =>
    `${provider} asked us to wait. Next try at ${time}.`,
  authExpired: "Authentication expired",
  authExpiredToken: (provider: string) => `Reconnect ${provider} to resume.`,
  authExpiredCli: (cli: string, device: string) =>
    `Sign in to the ${cli} CLI again on ${device}. AnyDash resumes automatically.`,
  lastValue: (value: string, time: string) => `Last value ${value} · ${time}`,
  // Without a count (another member's shared widget) only the state shows.
  partialAggregation: (records?: number) =>
    records === undefined
      ? "Partial aggregation"
      : `Partial aggregation · ${records} records skipped`,
  partialAggregationDetail: (device: string) =>
    `Update the collector on ${device} to read them.`,
  cannotBeCalculated: "Can’t be calculated",
  cannotBeCalculatedDetail: (model: string) =>
    `No public price for ${model} at that time. Token counts are still shown.`,
  sharingEnded: "Sharing ended",
  sharingEndedDetail:
    "The owner stopped sharing this widget. Its data was removed from this device.",
  removeWidget: "Remove widget",
  duplicateDevices: (devices: number) =>
    `Usage from ${devices} devices is combined.`,
  duplicateDevicesDetail:
    "If the same session files are copied to more than one device, their usage is counted once per device.",
  estimatedCost: (amount: string) => `Est. cost $${amount} (USD, estimated)`,
  estimatedCostDetail:
    "Based on public API prices. Not your subscription fee or bill.",
  updateRequired: "Update required",
  updateRequiredDetail:
    "This version of AnyDash is no longer supported. Update to keep syncing. Collection continues on this device.",
  limitReached: "Limit reached",
  limitReachedDetail: (limit: number, things: string) =>
    `You can have up to ${limit} ${things}. Delete one to add another.`,
  spaceUnavailable: "This space is unavailable",
  spaceUnavailableDetail:
    "The team is blocked or being deleted. Contact the team Owner.",

  percentLeft: (percent: string) => `${percent}% left`,
  percentUsedResets: (used: string, time: string) =>
    `${used}% used · Resets ${time}`,
  percentUsed: (used: string) => `${used}% used`,
  billedCost: (currency: string, provider: string) =>
    `${currency}, billed cost from ${provider}`,
  projectsReady: (count: number) =>
    `${count} ${count === 1 ? "project" : "projects"} ready`,

  connectDevice: "Connect a device to start",
  connectDeviceDetail:
    "Your personal dashboard uses usage collected on your computers. Install the AnyDash app or CLI, then approve the device here.",
  downloadApp: "Download the app",
  installCli: "Install the CLI",
  teamWithoutDevice: "Team dashboards work without a device.",
  noDashboards: "No dashboards yet",
  noDashboardsDetail: "Create a dashboard and add widgets for your providers.",
  createDashboard: "Create dashboard",
  dashboardLimit: "Up to 50 dashboards per space.",
  accessDenied: "You don’t have access to this dashboard",
  accessDeniedDetail:
    "Team dashboards are available to Owners, Admins and Members. Ask a team Owner or Admin for access.",
  goToDashboards: "Go to your dashboards",
  somethingWrong: "Something went wrong",
  somethingWrongDetail:
    "We couldn’t load this dashboard. Try again. If it keeps happening, report it with the details below.",
  tryAgain: "Try again",
  copyDetails: "Copy details",
  correlationId: "Correlation ID",
  deviceId: "Device ID",
  connectionId: "Connection ID",
  collector: "Collector",
  dashboardChanged: "This dashboard changed",
  dashboardChangedDetail:
    "Someone saved changes while you were editing. We loaded the latest version. Make your edit again.",
  continueEditing: "Continue editing",

  approveDevice: "Approve this device?",
  enterCode: "Enter the code shown on the device.",
  expiresIn: (time: string, attempts: number) =>
    `Expires in ${time} · ${attempts} attempts left`,
  approve: "Approve",
  notMyDevice: "This isn’t my device",
  codeInvalid: "This code no longer works",
  codeInvalidBefore:
    "The code expired, was already used, or was entered wrong 5 times. Run ",
  codeInvalidCommand: "anydash login",
  codeInvalidAfter: " again, or restart sign-in in the app, to get a new code.",

  shareTitle: (team: string) => `Share this widget with ${team}?`,
  shareBefore: "Team members will see only this widget’s display data: ",
  shareAfter:
    ". They can’t see your connection, credentials or any other data. Only you can change its period, filters or sharing.",
  shareWidget: "Share widget",
  deleteDashboardTitle: (name: string) => `Delete “${name}”?`,
  deleteDashboardDetail: (widgets: number) =>
    `The dashboard and its ${widgets} widgets are removed for everyone in this space. Your connections and usage data are kept.`,
  deleteDashboard: "Delete dashboard",
  deleteDataTitle: "Delete all AnyDash data?",
  deleteDataBefore:
    "This deletes your devices, connections, usage history, personal dashboards and shared widgets. Access stops now; deletion completes within 24 hours. Session files on your devices are not deleted. Your Delino account stays — ",
  deleteDataLink: "delete your account",
  deleteDataAfter: " separately.",
  deleteData: "Delete AnyDash data",
  connectProvider: (provider: string) => `Connect ${provider}`,
  vercelWarning:
    "Vercel tokens can also make changes, because Vercel has no read-only token. AnyDash only reads data. The token stays on this device.",
  teamScopedToken: "Team-scoped token",
  connect: "Connect",

  nativeUpdated: (time: string) => `Updated ${time}`,
  nativeSharingEnded: "Open AnyDash to choose another widget.",
} as const;

// Copy for surfaces the design record does not cover. These follow the
// record's tone and patterns and are listed as design gaps for review.
export const ExtraCopy = {
  signInTitle: "Sign in to AnyDash",
  signInDetail:
    "See AI subscription limits, local usage from your devices and deployment status in one dashboard.",
  signIn: "Sign in",
  signingIn: "Signing in…",
  connecting: "Connecting…",
  settings: "Settings",
  back: "Back to dashboards",
  personal: "Personal",
  dashboards: "Dashboards",
  chooseSpace: "Choose a space",
  chooseDashboard: "Choose a dashboard",
  dashboardName: "Dashboard name",
  renameDashboard: "Rename dashboard",
  save: "Save",
  saving: "Saving…",
  close: "Close",
  addWidget: "Add widget",
  widgetType: "Widget type",
  presentation: "Presentation",
  period: "Period",
  breakdown: "Breakdown",
  source: "Data source",
  allDevices: "Usage from all your devices, including ones you add later.",
  noEligibleConnections: "Add a connection for this widget in Settings first.",
  noWidgets: "No widgets yet",
  noWidgetsDetail: "Add a widget to show usage on this dashboard.",
  editorHint:
    "Select a widget, then use the arrow keys to move it. Hold Shift with the arrow keys to resize it.",
  moveLeft: "Move left",
  moveRight: "Move right",
  moveUp: "Move up",
  moveDown: "Move down",
  wider: "Wider",
  narrower: "Narrower",
  taller: "Taller",
  shorter: "Shorter",
  stopSharing: "Stop sharing",
  stopSharingTitle: "Stop sharing this widget?",
  stopSharingDetail:
    "Team members lose access to its data now. Their apps remove it at the next refresh.",
  connectThisComputer: "Connect this computer",
  installCliTitle: "Install the CLI",
  installCliDetail:
    "Run one of these commands on the computer, then run anydash login and approve the device here.",
  approveInBrowser: "Approve in your browser",
  approveInBrowserDetail:
    "We opened the approval page. Enter this code there to connect this computer.",
  connectFailed: "The connection request ended. Try again to get a new code.",
  collectorUnavailable:
    "The AnyDash collector isn’t running. Quit and reopen AnyDash, then try again.",
  deviceApproved: "Device approved",
  deviceApprovedDetail:
    "Return to the device. Collection starts within a minute.",
  requestRejected: "Request rejected",
  requestRejectedDetail: "That device can’t connect with this code.",
  codeMismatch: "That code doesn’t match. Check the device and try again.",
  notFound: "This page doesn’t exist",
  notFoundDetail: "Check the link, or go back to your dashboards.",
  actionFailed: "That didn’t work. Try again.",
  timeZone: "Time zone",
  timeZoneDetail: "Every dashboard shows times in this time zone.",
  timeZoneHint: "An IANA time zone name, for example Asia/Seoul.",
  devices: "Devices",
  noDevices: "No devices yet. Connect a computer to collect usage.",
  online: "Online",
  lastSeen: "Last seen",
  preferred: "Preferred",
  setPreferred: "Set as preferred",
  syncNow: "Sync now",
  delete: "Delete",
  deleteDeviceTitle: "Delete this device?",
  deleteDeviceDetail:
    "Its usage data is removed and it stops collecting now. Deletion completes within 24 hours.",
  deleteDevice: "Delete device",
  connections: "Connections",
  connectionOk: "Connected",
  waitingFirstResult: "Waiting for first result",
  noRunningCollector: "No collector running",
  noRegisteredDevice: "No device registered",
  unknownState: "Unknown",
  cliResumes:
    "Sign in to the CLI again on a registered device. AnyDash resumes automatically.",
  removeFromComputer: "Remove from this computer",
  useOnComputer: "Use on this computer",
  addConnection: "Add connection",
  addConnectionElsewhere:
    "Add connections from the AnyDash desktop app or the CLI. Credentials stay on that device.",
  apiToken: "API token",
  vercelTeam: "Vercel team ID",
  vercelTeamHint: "Leave empty for a personal account.",
  localLoginDetail: (provider: string) =>
    `AnyDash reads your existing ${provider} CLI sign-in on this computer. Sign in to the CLI first.`,
  deleteConnectionTitle: (provider: string) =>
    `Delete this ${provider} connection?`,
  deleteConnectionDetail:
    "Its data and shared widgets stop working now. Deletion completes within 24 hours.",
  deleteConnection: "Delete connection",
  thisComputer: "This computer",
  collectorState: "Collector status",
  collectorRunning: "Running",
  collectorNotLoggedIn: "Not signed in",
  collectorOffline: "Offline",
  collectorInactive: "Sign-in required",
  lastSync: "Last local sync",
  lastUpload: "Last upload",
  pendingBuckets: "Pending usage records",
  keySource: "Encryption key",
  keyOs: "OS secure store",
  keyFile: "Key file",
  keyEnvironment: "Environment variable",
  startAtLogin: "Start AnyDash at login",
  on: "On",
  off: "Off",
  sessionPaths: "Session paths",
  pathFound: "Found",
  pathMissing: "Not found",
  disable: "Disable",
  enable: "Enable",
  addPath: "Add a session folder",
  addPathAction: "Add folder",
  rotateKey: "Rotate key",
  rotateKeyTitle: "Rotate the encryption key?",
  rotateKeyDetail:
    "AnyDash re-encrypts the credentials on this computer with a new key. If anything fails, the current key stays in place.",
  rotatedKey: (count: number) =>
    `Key rotated. ${count} credentials re-encrypted.`,
  appUpdates: "App updates",
  checkUpdates: "Check for updates",
  installUpdate: "Install update",
  installUpdateTitle: (version: string) => `Install AnyDash ${version}?`,
  installUpdateDetail:
    "The signature was verified. AnyDash restarts to finish, and collection resumes after the restart.",
  activity: "Activity",
  activityDetail: "Device and connection events from the last 30 days.",
  deleteDataSummary:
    "Delete your devices, connections, usage history, personal dashboards and shared widgets.",
  about: "About and support",
  reportIssue: "Report an issue on GitHub",
  noPersonalData: "Don’t include personal data in public issues.",
  reportSecurity: "Report a security vulnerability privately",
  signOut: "Sign out",
} as const;
