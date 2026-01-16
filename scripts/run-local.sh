#!/bin/bash
# Run the complete local development environment
# Usage: ./scripts/run-local.sh

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== Scalable Web3 Storage - Local Development ===${NC}"
echo ""

# Check prerequisites
check_prereqs() {
    echo -e "${YELLOW}Checking prerequisites...${NC}"

    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}Error: cargo not found. Install Rust first.${NC}"
        exit 1
    fi

    if ! command -v zombienet &> /dev/null; then
        echo -e "${RED}Error: zombienet not found.${NC}"
        echo "Install with: cargo install zombienet"
        echo "Or download from: https://github.com/paritytech/zombienet/releases"
        exit 1
    fi

    if ! command -v polkadot &> /dev/null; then
        echo -e "${RED}Error: polkadot not found.${NC}"
        echo "Download from: https://github.com/paritytech/polkadot-sdk/releases"
        exit 1
    fi

    if ! command -v polkadot-omni-node &> /dev/null; then
        echo -e "${RED}Error: polkadot-omni-node not found.${NC}"
        echo "Build with: cargo build --release -p polkadot-omni-node (from polkadot-sdk)"
        echo "Or download from: https://github.com/paritytech/polkadot-sdk/releases"
        exit 1
    fi

    echo -e "${GREEN}All prerequisites found!${NC}"
}

# Build the project
build_project() {
    echo ""
    echo -e "${YELLOW}Building project...${NC}"
    cargo build --release
    echo -e "${GREEN}Build complete!${NC}"
}

# Show instructions
show_instructions() {
    echo ""
    echo -e "${GREEN}=== Setup Complete ===${NC}"
    echo ""
    echo "Run the following commands in separate terminals:"
    echo ""
    echo -e "${YELLOW}Terminal 1 - Start blockchain (relay chain + parachain):${NC}"
    echo "  cd $(pwd)"
    echo "  zombienet spawn zombienet.toml"
    echo ""
    echo -e "${YELLOW}Terminal 2 - Start provider node (after blockchain is ready):${NC}"
    echo "  cd $(pwd)"
    echo "  export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    echo "  export CHAIN_RPC=ws://127.0.0.1:9944"
    echo "  cargo run --release -p storage-provider-node"
    echo ""
    echo -e "${YELLOW}Terminal 3 - Test the setup:${NC}"
    echo "  curl http://localhost:3000/health"
    echo ""
    echo -e "${GREEN}Polkadot.js Apps:${NC}"
    echo "  Relay chain: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900"
    echo "  Parachain:   https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944"
    echo ""
}

# Main
cd "$(dirname "$0")/.."

case "${1:-}" in
    --check)
        check_prereqs
        ;;
    --build)
        build_project
        ;;
    *)
        check_prereqs
        build_project
        show_instructions
        ;;
esac
