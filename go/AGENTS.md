# Go library ownership

`go/` owns independent Go client modules. Read the owning project index and package contract before changing a module.

- `clairic/`: [project index](../docs/project-clairic.md) and [client contract](../docs/packages-clairic-client-contract.md).
- Run Go formatting, tests and vet inside each affected module. Root Go commands do not traverse nested modules.
- Keep these modules independent of repository server dependencies and local Go workspace configuration.
