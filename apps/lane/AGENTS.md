# Lane working instructions

- Follow root and parent instructions.
- Read the [project index](../../docs/project-lane.md) and [application contract](../../docs/apps-lane-foundation.md) before changes.
- Shared ReactLynx source belongs in `src/`; native platform integration belongs in `native/`; browser host code belongs in `web/`.
- Keep app-specific build helpers in `scripts/`. Do not commit SDK binaries, generated platform projects, signing material or build output.
- Run app-local `pnpm test` for frontend changes. Record native build and device acceptance separately.
