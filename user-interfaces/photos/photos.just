# Photos dApp (Layer 1) — contract + headless M1 flow.
#
# Invoked as a module from the root justfile: `just photos <recipe>`
# (e.g. `just photos deploy`). Run `just photos` to list the recipes.
#
# `build` needs solc + resolc on PATH; `deploy`/`flow` need a running chain
# (+ provider, registered & accepting, e.g. after `just demo`); all need
# `pnpm install` in user-interfaces/.

# `justfile_directory()` is the root justfile's dir (repo root), even inside a
# module — so this is an absolute path to the pnpm workspace, independent of cwd.
ui := justfile_directory() / 'user-interfaces'
pkg := '@web3-storage/photos'
chain_ws := 'ws://127.0.0.1:2222'
provider_url := 'http://127.0.0.1:3333'

# Default recipe (runs on bare `just photos`): list this module's recipes.
[private]
default:
    @just --justfile '{{ source_file() }}' --list

# Compile Photos.sol → src/contract/Photos.json.
build:
    cd '{{ ui }}' && pnpm --filter {{ pkg }} build:contract

# Deploy the Photos contract once per network; prints its H160 address.
deploy seed='//Alice':
    cd '{{ ui }}' && pnpm --filter {{ pkg }} deploy-contract -- '{{ chain_ws }}' '{{ seed }}'

# Deploy → negotiate terms → createLibrary → assert (the headless M1 flow).
flow provider_seed='//Alice' client_seed='//Bob':
    cd '{{ ui }}' && pnpm --filter {{ pkg }} photos-flow -- '{{ chain_ws }}' '{{ provider_url }}' '{{ provider_seed }}' '{{ client_seed }}'
