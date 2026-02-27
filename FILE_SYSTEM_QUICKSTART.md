# File System Quick Start Guide

Quick commands to test and run the Layer 1 File System Interface.

## Prerequisites

Install `just`:
```bash
cargo install just
# or on macOS:
brew install just
```

## Quick Commands

### 🚀 Full Integration Test (Recommended for First Run)

Starts everything and runs the example:
```bash
just fs-integration-test
```

This will:
1. ✅ Start relay chain + parachain
2. ✅ Start provider node
3. ✅ Verify on-chain setup
4. ✅ Run file system example

**Expected output:** Complete file system workflow demonstration with drive creation, file uploads, downloads, and verification.

### 📋 Quick Demo (Infrastructure Already Running)

If you already have the infrastructure running:
```bash
# Terminal 1 - Start infrastructure
just start-services

# Terminal 2 - Run demo
just fs-demo
```

### 🧪 Testing

```bash
# Test file system client only
just fs-test

# Test with verbose logging
just fs-test-verbose

# Test all file system components (primitives + pallet + client)
just fs-test-all
```

### 🔧 Building

```bash
# Build file system components only
just fs-build

# Build entire project
just build
```

### 📚 Documentation

```bash
# Show documentation links
just fs-docs
```

## Manual Workflow

If you prefer to run each step manually:

### Step 1: Setup (One-time)

```bash
just setup
```

This downloads binaries and builds the project.

### Step 2: Start Infrastructure

**Terminal 1 - Blockchain:**
```bash
just start-chain

# Wait for:
# ✓ Relay chain: ws://127.0.0.1:9900
# ✓ Parachain: ws://127.0.0.1:9944
```

**Terminal 2 - Provider:**
```bash
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
export CHAIN_RPC=ws://127.0.0.1:9944
cargo run --release -p storage-provider-node

# Wait for:
# ✓ Storage provider listening on http://0.0.0.0:3000
```

### Step 3: Verify Setup

**Terminal 3:**
```bash
# Verify blockchain
bash scripts/check-chain.sh

# Verify provider
curl http://localhost:3000/health

# Verify on-chain setup
bash scripts/verify-setup.sh
```

### Step 4: Run Example

```bash
just fs-example
```

## Example Output

```
🚀 File System Client - Basic Usage Example

============================================================

📡 Step 1: Connecting to blockchain and provider...
✅ Connected successfully!

📁 Step 2: Creating a new drive...
✅ Drive created with ID: 0
   Name: My Documents
   Capacity: 10 GB
   Duration: 500 blocks

📂 Step 3: Creating directory structure...
   Creating /documents...
   ✅ Created /documents
   Creating /documents/work...
   ✅ Created /documents/work
   Creating /photos...
   ✅ Created /photos

📝 Step 4: Uploading files...
   Uploading /README.md (92 bytes)...
   ✅ Uploaded /README.md
   Uploading /documents/work/report.txt (75 bytes)...
   ✅ Uploaded /documents/work/report.txt
   Uploading /documents/notes.txt (93 bytes)...
   ✅ Uploaded /documents/notes.txt

📋 Step 5: Listing directory contents...

   Contents of /:
   📁 documents (0 bytes)
   📁 photos (0 bytes)
   📄 README.md (92 bytes)

   Contents of /documents:
   📁 work (0 bytes)
   📄 notes.txt (93 bytes)

   Contents of /documents/work:
   📄 report.txt (75 bytes)

⬇️  Step 6: Downloading and verifying files...

   Downloading /README.md...
   ✅ Downloaded 92 bytes
   ✅ Content verified!
   Content preview: # My Documents

Welcome to my decentralized fil

   Downloading /documents/work/report.txt...
   ✅ Downloaded 75 bytes
   ✅ Content verified!
   Content:
   Q4 2024 Report

   Revenue: $1M
   Growth: 50%

============================================================

🎉 Example completed successfully!

📊 Summary:
   ✅ Created drive: 0
   ✅ Created 3 directories
   ✅ Uploaded 3 files
   ✅ Listed directory contents
   ✅ Downloaded and verified files

💡 Next steps:
   - Try clearing the drive: clear_drive()
   - Try deleting the drive: delete_drive()
   - Explore more file operations
   - Check the on-chain state via polkadot.js
```

## Troubleshooting

### "Connection failed" Error

**Problem:** Cannot connect to blockchain

```bash
# Check blockchain status
bash scripts/check-chain.sh

# Restart blockchain
just start-chain
```

### "Provider unavailable" Error

**Problem:** Cannot reach provider

```bash
# Check provider status
curl http://localhost:3000/health

# Restart provider (in separate terminal)
export PROVIDER_ID=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
export CHAIN_RPC=ws://127.0.0.1:9944
cargo run --release -p storage-provider-node
```

### Ports Already in Use

```bash
# Find and kill processes
lsof -ti:9944 | xargs kill  # Parachain
lsof -ti:9900 | xargs kill  # Relay chain
lsof -ti:3000 | xargs kill  # Provider
```

### Clean Start

```bash
# Kill all processes
pkill -f polkadot
pkill -f storage-provider-node
pkill -f zombienet

# Rebuild
just build

# Start fresh
just fs-integration-test
```

## Available Just Commands

### File System Commands

| Command | Description |
|---------|-------------|
| `just fs-integration-test` | Full integration test (starts everything) |
| `just fs-demo` | Quick demo (requires running infrastructure) |
| `just fs-example` | Run basic_usage.rs example |
| `just fs-test` | Run unit tests |
| `just fs-test-verbose` | Run tests with logging |
| `just fs-test-all` | Test all file system components |
| `just fs-build` | Build file system components |
| `just fs-clean` | Clean build artifacts |
| `just fs-docs` | Show documentation links |

### Infrastructure Commands

| Command | Description |
|---------|-------------|
| `just setup` | One-time setup (download binaries + build) |
| `just start-chain` | Start blockchain only |
| `just start-services` | Start blockchain + provider |
| `just health` | Check provider health |
| `just build` | Build entire project |

## Web UIs

Once infrastructure is running, access these UIs:

- **Relay Chain**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900
- **Parachain**: https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9944
- **Provider Health**: http://127.0.0.1:3000/health

## Next Steps

1. **Read the walkthrough**: `docs/filesystems/EXAMPLE_WALKTHROUGH.md`
2. **Explore the API**: `docs/filesystems/API_REFERENCE.md`
3. **Build your app**: Use the example as a template
4. **Modify the example**: Try different drive parameters, files, and operations

## Documentation

- **[User Guide](docs/filesystems/USER_GUIDE.md)** - Complete user workflows
- **[Example Walkthrough](docs/filesystems/EXAMPLE_WALKTHROUGH.md)** - Step-by-step guide
- **[API Reference](docs/filesystems/API_REFERENCE.md)** - Complete API docs
- **[Client README](storage-interfaces/file-system/client/README.md)** - SDK documentation
- **[File System Overview](docs/filesystems/FILE_SYSTEM_INTERFACE.md)** - Architecture and design

## Support

For issues:
1. Check logs: `RUST_LOG=debug just fs-example`
2. Verify setup: `bash scripts/verify-setup.sh`
3. Review troubleshooting section above
4. Check main documentation: `docs/README.md`
