# Lane Application Foundation

## Scope

`apps/lane` owns the Lane client scaffold for Android, iOS, macOS, Windows and
web. It contains an empty ReactLynx root and platform hosts. Product UI, account
integration, backend services and signed store publication are outside this scaffold.

## Runtime and Language

Use Node.js 24, TypeScript, ReactLynx 0.129 and Rspeedy 0.18.3. Native hosts use
Lynx 4.1.0; Android PrimJS uses 4.1.1. Web uses web-core 0.26.5. Kotlin/Gradle,
Objective-C/CocoaPods and C++17/CMake own native integration. Desktop uses Clay
on macOS and Windows; Linux can use the browser build.

## Users and Operators

Developers build and run the empty client. Maintainers configure signing and
platform distribution separately.

## Interfaces and Contracts

The name is `Lane`; the identifier is `io.delino.lane`. `src/index.tsx` registers
one shared root. `pnpm build` emits `dist/main.lynx.bundle`, the browser bundle,
a local browser runtime and license notices. `pnpm build:frontend` is equivalent.

`pnpm dev` injects Infisical development settings and registers the `lane`
Portless route at `https://lane.localhost`, with fixed loopback port 46325 and
conflict rejection. It serves the browser host and shared bundles through Rspeedy.
Native builds embed the local production bundle and do not fetch remote templates.

- Desktop: supply an extracted official Lynx 4.1.0 SDK with `LANE_LYNX_SDK`.
  Run `pnpm build:desktop`, then `pnpm dev:desktop` to launch the built host.
  macOS requires Xcode command-line tools and CMake; Windows requires Visual
  Studio C++ tools and CMake. Desktop opens a resizable window and exits after
  the last window closes.
- Android: install JDK 17 or 21 and Android SDK 35. The committed Gradle
  wrapper selects 8.11.1. Run
  `pnpm build:android` for an unsigned release APK. Import `native/android` in
  Android Studio for installation and debugging.
- iOS: install Xcode, XcodeGen and CocoaPods. Run `pnpm mobile:ios:init` to
  generate the Xcode project and install pods after building the shared bundle.
  Run `pnpm build:ios` for an unsigned simulator build. Open the generated
  workspace for device development and signing.

Web output is a static Cloudflare Pages candidate. Upload `apps/lane/dist` after
building from the root with `pnpm --filter lane build`. No domain, account,
release workflow, signing identity or store registration is provisioned.

## Storage

There is no product persistence. `dist`, native build output, generated Xcode
projects, CocoaPods and local SDK paths remain ignored. Native packages embed
only the compiled template, required runtime resources and license notices.

## Security

The scaffold has no service credentials, network permissions or application
capabilities. Keep SDK downloads and signing material outside tracked source.
Development commands follow the root Infisical contract. SDK libraries and
notices must remain together when packaging native applications.

## Logging

Build helpers emit structured start, completion and failure events. Native hosts
report startup and template-load failures without user content. Platform runtime
logs remain available through the platform's development tools.

## Build and Test

Run `pnpm test` in `apps/lane` for TypeScript checks and both shared bundle builds.
Affected frontend CI runs on Ubuntu. Native compilation, launch, resizing,
lifecycle, signing, installation and store submission need separate platform
validation. No macOS runner is added to PR, main-push or affected manual CI.
Remove generated `dist` after validation.

## Dependencies and Integrations

The ReactLynx toolchain versions match the existing Lynx UI workspace. The empty
root does not need UI primitives. Native SDKs are platform dependencies, not
vendored source. Desktop uses an operator-supplied SDK; Android resolves Maven
artifacts; iOS resolves CocoaPods. Preserve original third-party licenses. `native/licenses` retains the Lynx 4.1.0
license and notice for desktop distributions. Mobile dependency managers retain
their dependency notices; include them in signed release packaging.

## Change Triggers

Keep this contract, the project index, documentation catalog and affected CI
selection aligned. Update instructions only when ownership or development rules
change. Record validation limits in PR and CI evidence.

## References

- [Project index](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Official Lynx integration](https://lynxjs.org/guide/start/integrate-with-existing-apps.html)
- [Official desktop SDK release](https://github.com/lynx-family/lynx/releases/tag/4.1.0)
