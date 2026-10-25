# Lane working instructions

- Follow root and parent instructions.
- Read the [project index](../../docs/project-lane.md) and [application contract](../../docs/apps-lane-foundation.md) before changes.
- Shared ReactLynx source belongs in `src/`; native platform integration belongs in `native/`; browser host code belongs in `web/`.
- Keep app-specific build helpers in `scripts/`. Do not commit SDK binaries, generated platform projects, signing material or build output.
- Run app-local `pnpm test` for frontend changes. Record native build and device acceptance separately.

- Read the [local-store contract](../../docs/apps-lane-local-store-contract.md) and [protocol contract](../../docs/protos-lane-v1-contract.md) for host persistence and cross-language interfaces. `native/store` owns shared SQL and native store support; platform registration remains in each host.

- Desktop uses Lynxtron; `native/desktop/main.mjs` owns application lifecycle and message dispatch. CMake builds only the Node-API storage/OS module. Keep mobile SDK integration separate. Use `scripts/desktop.mjs` for desktop packaging and launch; do not add a manual desktop Lynx SDK prerequisite.
- Run desktop native validation with an explicit scratch data root and a `LANE_NATIVE_TESTING=1` build. Production builds disable test-root injection. Keep Lynxtron's original runtime licenses and framework symlinks intact when packaging.
