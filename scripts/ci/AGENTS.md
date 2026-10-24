# scripts/ci working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `scripts/ci/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts before changing behavior, including cross-domain consumers.
- Propagate the event's exact affected comparison to selectors, protocol checks and affected-validation evidence. Formatting checks run across all tracked supported files without change detection. Do not infer full validation from a manual event alone.
- Preserve selected-job failure, cancellation and skipped-result rejection in the final result check. Keep native/account acceptance distinct from fixtures and builds.

## Owning contracts

- [Repository workflow](../../docs/repository-workflow-contract.md)
- [Perfine fixture and adapter validation](../../docs/repository-workflow-contract.md#perfine-repository-validation)

- Perfine CLI acceptance uses `perfine-acceptance.mjs <language>` and the six-language Linux matrix. Preserve raw failure evidence and label local emulation explicitly; do not enable the emulation override in CI.
