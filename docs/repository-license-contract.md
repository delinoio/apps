# Repository License Contract

## Ownership

Delino-owned source and future distributions use Apache-2.0, except `servers/` as described below. Root `LICENSE` contains the complete terms and `NOTICE` identifies the copyright holder. New repository-owned manifests must declare Apache-2.0, or `FSL-1.1-ALv2` under `servers/`.

## Servers

Source under `servers/` uses the Functional Source License, Version 1.1, ALv2 Future License (`FSL-1.1-ALv2`). `servers/LICENSE` contains the complete terms; it overrides the root Apache-2.0 license for that directory. Each version converts to Apache-2.0 on the second anniversary of the date it is made available. Server source headers and manifests declare `FSL-1.1-ALv2`. Keep the license file text unchanged except for the notice.

Shared code that apps, packages, crates or command tools consume must live outside `servers/` under Apache-2.0. Moving code between `servers/` and Apache-2.0 directories changes its license and requires an explicit relicensing decision.

## Imported material

Imported agent skills retain their upstream licenses, attribution and notices. A repository-wide default does not relicense imported material. Keep original licenses and copyright notices for any future imported source, fonts and assets.

## Distribution

Future distributions must include the complete applicable license terms and notices. Validate package contents in the owning project's distribution checks. This repository publishes no runtime packages or release artifacts yet.
