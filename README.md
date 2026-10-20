# Delino Apps

DevBase, Delino website, DeliDev, DeliTools, FixKey, Git Tracker, shared protocols and consumer examples, and the Delcore and Git Tracker servers.

This repository contains clients and servers in one checkout. Repository-owned files under `servers/` use [FSL-1.1-ALv2](servers/LICENSE), with Apache-2.0 available two years after each version is first publicly distributed. Other repository-owned files use [Apache-2.0](LICENSE). Third-party licenses and notices remain in effect.

Contributors can change clients, protocols and servers together. Follow the owning scoped instructions and run the affected checks with disposable databases and mocked providers. Production credentials are not required for fixture-based contributions.

Maintainer guidance: [documentation catalog](docs/README.md), [validation workflow](docs/repository-workflow-contract.md), [protocol contract](docs/protos-delcore-v1-contract.md) and [directory boundaries](docs/repository-boundary-contract.md). For protocol changes, update affected contracts and describe interface changes and validation in the pull request.
