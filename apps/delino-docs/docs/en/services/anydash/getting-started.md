# Get started with AnyDash

To start, sign in, install a collector on a computer, approve that computer, and add your provider connections.

## Sign in

Sign in with your Delino account on every platform: the web app at `https://anydash.delino.io`, the desktop app and the mobile app. If you sign out of an app, it removes its sign-in tokens and cached dashboard data from that device.

## Personal and team spaces

AnyDash organizes dashboards in spaces:

- **Personal** is your own space. Its dashboards show data from your own devices and connections. The personal dashboard is available only after you connect at least one collector device. Until then, the personal space shows "Connect a device to start".
- **Team spaces** are your Delino teams. You can open team dashboards without a collector device of your own. Owners and Admins can create, delete, edit and view team dashboards. Members can edit and view them. A user who has only the Billing role, or no role in the team, cannot see the team space.

Each space can have up to 50 dashboards, and each dashboard can have up to 100 widgets. Select your time zone in **Settings**. Every dashboard shows times in that time zone.

## Supported platforms

| Surface                   | Supported versions                                                                                                                                                | Role                                    |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------- |
| Web                       | The latest 2 major versions of Chrome, Edge, Firefox and Safari                                                                                                   | View and edit dashboards                |
| macOS app                 | macOS 14 or later, Apple silicon (arm64) and Intel (x64)                                                                                                          | View and edit dashboards; collector     |
| Windows app               | Windows 11 x64. A Windows 11 arm64 app is available only if the desktop framework supports a native arm64 build. Otherwise, use the CLI and the web app on arm64. | View and edit dashboards; collector     |
| iOS and iPadOS app        | iOS 16 or later, phone and tablet                                                                                                                                 | View and edit dashboards (no collector) |
| Android app               | Android 10 (API level 29) or later, phone and tablet                                                                                                              | View and edit dashboards (no collector) |
| Linux                     | No desktop app. Use the CLI collector and the web app.                                                                                                            | Collector through the CLI               |
| CLI collector (`anydash`) | macOS 14 or later (arm64, x64); Windows 11 (x64, arm64); Ubuntu 24.04 and 26.04 and Debian 12 and 13 (x64, arm64)                                                 | Collector                               |

Native widgets are available on the macOS desktop and Notification Center, the Windows Widgets Board, and the iOS and Android home screens. A native widget shows a summary of one dashboard widget that you select, and the time of its last refresh. Tap or click it to open that dashboard in the app. You edit widgets only in the app. The operating system controls how often a native widget refreshes.

## Install a collector

A collector reads local session usage and calls your providers. Install it on each computer that you use for Codex or Claude Code, and on at least one computer that can poll your other connections. Mobile apps and the web app do not collect data.

### Desktop app

Download the macOS or Windows app from the download links on `https://anydash.delino.io`. The links point to the signed installers on GitHub Releases. The macOS app is signed and notarized by Apple. The desktop app includes its own collector, so you do not need the CLI on the same computer.

The desktop app checks for updates. It installs an update only after it verifies the update's signature and you confirm the installation.

### CLI

On macOS or Linux, run:

```sh
curl -fsSL https://anydash.delino.io/install.sh | sh
```

On Windows, run in PowerShell:

```powershell
irm https://anydash.delino.io/install.ps1 | iex
```

The install scripts select the archive for your OS and CPU, download it and its `SHA256SUMS` file from GitHub Releases, and verify the checksum before they install anything. They install `anydash` for your user only, without administrator rights: into `~/.local/bin` on macOS and Linux, and into `%LOCALAPPDATA%\Programs\AnyDash` on Windows. If the checksum does not match, or your platform is not supported, the script stops with an error and installs nothing. Make sure that the install directory is on your `PATH`.

You can also install the CLI with npm:

```sh
npm install -g @delinoio/anydash
anydash --version
```

npm installs only the executable that matches your OS and CPU. Keep optional dependencies enabled; do not use `--omit=optional`. Before each run, the launcher verifies the installed executable's version, platform and checksum. If the executable is missing or does not match, the launcher stops and tells you to reinstall. It never downloads a replacement.

To update the CLI, run the install script again or run `npm update -g @delinoio/anydash`. The CLI has no self-update command. `anydash status` and the collector logs tell you when a new version is available.

## Connect a device

You approve each collector device in your browser with a connection code.

1. Start the connection on the computer:
   - In the desktop app, open the personal space and select **Connect this computer**. The app opens the approval page in your browser and shows a code.
   - With the CLI, run `anydash login`. It prints the approval URL and the code, and opens the browser when the computer has a graphical session. On a server without a browser, open the URL on another device. Use `anydash login --device-name NAME` to choose the device name; the default is the host name.
2. Sign in on the approval page if necessary. The page "Approve this device?" shows the device name, platform and collector version. Check that they match your computer.
3. Enter the code shown on the device, and select **Approve**. If you did not start this request, select **This isn't my device**.
4. Return to the device. Collection starts within a minute.

A connection code expires after 10 minutes and can be used only once. After 5 wrong entries, the code stops working and the page shows "This code no longer works". Run `anydash login` again, or restart sign-in in the app, to get a new code.

Each device belongs to the person who approved it. You can connect up to 20 collector devices.

On its first sync, the collector reads the last 90 days of local Codex and Claude Code sessions. Claude Code deletes its session files after 30 days by default, so AnyDash can read only the files that still exist.

## Run the collector

Each computer runs one collector for each OS user. The desktop app and the CLI share it, so usage is never collected twice on one computer.

- **Desktop app:** the collector runs in the background. If you close the window, collection continues and AnyDash stays in the menu bar (macOS) or the notification area (Windows). Select **Quit** to stop the app and the collector that it started. To start AnyDash when you log in, turn on **Start AnyDash at login** in **Settings**. This setting is off by default.
- **CLI:** run `anydash run` to collect in the foreground, or run `anydash service install` to start the collector automatically when you log in. See the [CLI reference](/services/anydash/cli#service-install-and-service-uninstall).

While the collector runs and is online, it syncs local usage every minute and polls external providers every 5 minutes. If no collector runs on any of your devices, external metrics stop refreshing.

## Add provider connections

Add connections in the desktop app (**Settings**, then **Connections**, then **Add connection**) or with `anydash connect`. The web and mobile apps cannot add connections, because credentials stay on the collector device. You can have up to 50 external connections. See [Provider connections](/services/anydash/providers) for the token or sign-in that each provider needs.

Codex and Claude Code local session usage needs no connection. The collector finds the default session folders automatically. Use `anydash paths` or **Settings**, then **Session paths**, to add or disable folders.

## Create dashboards and widgets

Select **Create dashboard**, then **Add widget**, and choose a widget type and a data source. In edit mode, you can move and resize widgets and change their title and presentation. With a keyboard, select a widget and use the arrow keys to move it; hold Shift with the arrow keys to resize it.

When you add your own widget to a team dashboard, AnyDash asks you to confirm what you share. See [team sharing](/services/anydash/data#team-sharing).

## Log out and reattach a device

Run `anydash logout` to log the collector out. Logout deletes the device credentials from the computer and stops collection and upload. AnyDash keeps the device registration and its data. When you run `anydash login` again on the same computer and approve it, AnyDash reattaches the same device instead of creating a new one.

If a device uploads nothing for 90 days, its sign-in expires and you must run `anydash login` again. To remove a device and its usage data permanently, delete it in **Settings**, then **Devices**. See [deletion](/services/anydash/data#deletion).
