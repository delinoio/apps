# Delino Apps

DevBase, Delino website, DeliDev, DeliTools, Git Tracker, shared protocols and consumer examples, and the Delcore and Git Tracker servers.

This repository contains clients and servers in one checkout.

Contributors can change clients, protocols and servers together. Follow the owning scoped instructions and run the affected checks with disposable databases and mocked providers. Production credentials are not required for fixture-based contributions.

Maintainer guidance: [documentation catalog](docs/README.md), [validation workflow](docs/repository-workflow-contract.md), [protocol contract](docs/protos-delcore-v1-contract.md) and [directory boundaries](docs/repository-boundary-contract.md). For protocol changes, update affected contracts and describe interface changes and validation in the pull request.

## Licensing

- **All repository-owned files under `servers/`: [FSL-1.1-ALv2](servers/LICENSE).** This includes source, tests, migrations, configuration examples, package metadata and documentation, even when a file has no license header.
- **Repository-owned files outside `servers/`: [Apache-2.0](LICENSE).** This includes clients, shared protocol schemas and generated bindings. Including protocol code in a server binary does not change the license of either component.
- **Third-party code and assets:** their original licenses and copyright notices remain in effect.

FSL permits use, modification and redistribution for its defined Permitted Purposes, including internal use. It restricts making the software available to others in a commercial product or service that constitutes a Competing Use, as defined in the [full terms](servers/LICENSE). Server source is available, but it is not initially offered under an unrestricted open-source license.

For each server version, the additional Apache-2.0 grant takes effect on the second anniversary of the date Delino makes that version available under FSL. Each newer version has its own two-year period. The root Apache-2.0 license and package metadata do not grant an immediate Apache-2.0 alternative for server files.

Redistributions must retain the applicable license terms and copyright notices. Server distributions include the server license and notices, plus the Apache-2.0 and third-party notices for any included components. These statements summarize the applicable licenses; the linked license texts govern.
