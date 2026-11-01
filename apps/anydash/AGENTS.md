# AnyDash working instructions

- Follow root and parent instructions.
- Read the [project index](../../docs/project-anydash.md), [application foundation](../../docs/apps-anydash-foundation.md) and [protocol contract](../../docs/protos-anydash-v1-contract.md) before changes.
- Shared ReactLynx source belongs in `src/`; the browser host in `web/`; native hosts in `native/` (`desktop`, `ios`, `android`); separate native widget hosts and their shared decoders in `native/widgets/`; app build helpers in `scripts/`.
- Keep the approved English copy in `src/copy.ts`. Strings outside `Copy` follow its patterns and need design approval before release.
- Hosts implement the `AnyDashHost` contract in `src/host.ts`; tokens stay in the host and never reach the Lynx bundle. Desktop collector calls use `LocalCollectorService` over local IPC only.
- Run app-local `pnpm test` for client changes and `pnpm test:browser` for web host changes. Regenerate widget decoder fixtures with `pnpm fixtures:widgets` when the snapshot fixtures change.
- Do not commit SDK binaries, generated platform projects, signing material, update keys or build output. Record native builds and device acceptance separately from bundle and fixture checks.
