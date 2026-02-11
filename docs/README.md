# Documentation Index

Welcome to the Scalable Web3 Storage documentation! This guide will help you navigate through all available documentation.

## 📚 Documentation Structure

```
docs/
├── getting-started/          # Quick start guides
├── testing/                  # Testing guides and procedures
├── reference/                # API references and calculators
└── design/                   # Architecture and design documents
```

### 🤖 For Claude Code and Contributors

**[CLAUDE.md](../CLAUDE.md)** - Essential reference for Claude Code AI and contributors

- Complete project overview and architecture
- Build, test, and run commands
- Development workflow and best practices
- Code review guidelines (Parity standards)
- Security considerations and testing requirements
- Common issues and solutions

**Start here if you're contributing code or using Claude Code to work on this project.**

---

## 🚀 Getting Started

Perfect for new users who want to get up and running quickly.

### [Quick Start Guide](./getting-started/QUICKSTART.md)
**Get running in 5 minutes!**

- One-command setup with `just`
- Start blockchain network and provider node
- Basic testing workflow
- Common troubleshooting

**Start here if you're new to the project.**

---

## 🧪 Testing

Comprehensive testing guides for developers and QA.

### [Manual Testing Guide](./testing/MANUAL_TESTING_GUIDE.md)
**Complete step-by-step testing workflow**

- Prerequisites setup with automated downloads
- 15 detailed testing steps
- Provider registration and configuration
- Bucket creation and management
- Storage agreements lifecycle
- Data upload/download testing
- Checkpoint creation
- Challenge and slashing mechanism
- Replica synchronization
- Performance testing
- Troubleshooting guide

**Use this for comprehensive system testing.**

---

## 📖 Reference Documentation

Detailed API references and calculation tools.

### [Extrinsics Reference](./reference/EXTRINSICS_REFERENCE.md)
**Complete pallet API documentation**

- All 24 extrinsics with exact parameters
- Provider management (register, configure, deregister)
- Bucket operations (create, manage members, freeze)
- Agreement lifecycle (request, accept, extend, burn)
- Checkpoint and challenge system
- Well-known test account IDs
- Common workflows
- Error reference
- Runtime configuration

**Essential reference for blockchain interactions.**

### [Payment Calculator](./reference/PAYMENT_CALCULATOR.md)
**Calculate storage agreement payments**

- Payment formula and examples
- Common value references (sizes, durations, decimals)
- Quick reference tables
- JavaScript and Python calculators
- Step-by-step calculation guide
- Common mistakes and solutions

**Use this to avoid payment-related errors.**

---

## 🏗️ Design & Architecture

High-level design documents and implementation details.

### [Scalable Web3 Storage Design](./design/scalable-web3-storage.md)
**High-level architecture and rationale**

- Design philosophy
- Game-theoretic approach
- System components
- Trust model
- Economic incentives

**Read this to understand the "why" behind the system.**

### [Implementation Details](./design/scalable-web3-storage-implementation.md)
**Technical implementation specifications**

- On-chain interface (pallet extrinsics)
- Off-chain interface (provider HTTP API)
- Data structures and storage
- MMR (Merkle Mountain Range) design
- Challenge mechanism
- Replica synchronization

**Read this for implementation details.**

### [Storage Marketplace](./design/marketplace.md)
**Provider discovery and matching system**

- Provider capacity declaration
- Capacity enforcement rules
- Storage requirements specification
- Matching algorithm and scoring
- Discovery client SDK
- Economic model and incentives
- Security considerations

**Read this to understand provider discovery and matching.**

---

## 🎯 Quick Navigation

### By User Type

#### **New User - First Time Setup**
1. [Quick Start Guide](./getting-started/QUICKSTART.md) - Get running fast
2. [Manual Testing Guide](./testing/MANUAL_TESTING_GUIDE.md) - Understand the system

#### **Developer - Building Applications**
1. [Client SDK Documentation](../client/README.md) - SDK usage
2. [Extrinsics Reference](./reference/EXTRINSICS_REFERENCE.md) - Blockchain API
3. [Payment Calculator](./reference/PAYMENT_CALCULATOR.md) - Cost estimation

#### **Provider Operator - Running Storage**
1. [Quick Start Guide](./getting-started/QUICKSTART.md) - Setup environment
2. [Manual Testing Guide](./testing/MANUAL_TESTING_GUIDE.md) - Provider registration
3. [Extrinsics Reference](./reference/EXTRINSICS_REFERENCE.md) - Provider API

#### **QA/Tester - System Validation**
1. [Manual Testing Guide](./testing/MANUAL_TESTING_GUIDE.md) - Full test suite
2. Scripts: `scripts/quick-test.sh` - Automated tests
3. Scripts: `scripts/verify-setup.sh` - Setup verification

#### **Researcher/Architect - Understanding Design**
1. [Design Document](./design/scalable-web3-storage.md) - Architecture
2. [Implementation Details](./design/scalable-web3-storage-implementation.md) - Technical specs

---

## 🔧 Related Documentation

### For Contributors & AI
- [CLAUDE.md](../CLAUDE.md) - Project overview, commands, and code review guidelines
- `.claude/commands/review-pr.md` - PR review command for Claude Code
- `.claude/skills/test/pallet.md` - Pallet testing skill for Claude Code

### Client SDK
- [Client README](../client/README.md) - SDK overview and examples
- [Integration Guide](../client/INTEGRATION.md) - Integrating with applications

### Scripts & Tools
- `scripts/quick-test.sh` - Automated basic tests
- `scripts/verify-setup.sh` - On-chain setup verification
- `scripts/check-chain.sh` - Blockchain health check
- `justfile` - Development commands (run `just --list`)

---

## 📋 Common Tasks

### Setting Up Development Environment
```bash
# Install dependencies and build
just setup

# Start blockchain + provider
just start-chain     # Terminal 1
just start-provider  # Terminal 2

# Verify setup
bash scripts/verify-setup.sh
```
See: [Quick Start Guide](./getting-started/QUICKSTART.md)

### Registering a Provider
1. Register on-chain with stake
2. Configure provider settings
3. Start provider node

See: [Manual Testing Guide](./testing/MANUAL_TESTING_GUIDE.md#step-4-register-storage-providers-on-chain)

### Creating Storage Agreements
1. Create bucket
2. Request agreement with payment calculation
3. Provider accepts agreement

See: [Payment Calculator](./reference/PAYMENT_CALCULATOR.md) for payment math

### Running Tests
```bash
# Basic automated tests
just demo

# Verify on-chain setup
bash scripts/verify-setup.sh

# Check system health
just health
```

---

## 🐛 Troubleshooting

### Common Issues

| Issue | Solution | Documentation |
|-------|----------|---------------|
| Chain not responding | Start zombienet | [Quick Start](./getting-started/QUICKSTART.md#troubleshooting) |
| Insufficient stake | Use 1000 tokens minimum | [Quick Start](./getting-started/QUICKSTART.md#troubleshooting) |
| PaymentExceedsMax | Calculate correct payment | [Payment Calculator](./reference/PAYMENT_CALCULATOR.md) |
| Upload fails | Complete on-chain setup | [Manual Testing](./testing/MANUAL_TESTING_GUIDE.md#step-7-create-a-bucket) |

### Getting Help

1. Check [Quick Start Troubleshooting](./getting-started/QUICKSTART.md#troubleshooting)
2. Review [Error Reference](./reference/EXTRINSICS_REFERENCE.md#error-reference)
3. Run verification: `bash scripts/verify-setup.sh`

---

## 📊 Documentation Status

| Document | Status | Last Updated | Completeness |
|----------|--------|--------------|--------------|
| Quick Start Guide | ✅ Ready | Current | Complete |
| Manual Testing Guide | ✅ Ready | Current | Complete |
| Extrinsics Reference | ✅ Ready | Current | Complete |
| Payment Calculator | ✅ Ready | Current | Complete |
| Design Document | ✅ Ready | Current | Complete |
| Implementation Details | ✅ Ready | Current | Complete |
| Storage Marketplace | ✅ Ready | Current | Complete |

---

## 🤝 Contributing to Documentation

When adding or updating documentation:

1. **Getting Started** - Add to `getting-started/` for quick setup guides
2. **Testing** - Add to `testing/` for test procedures and guides
3. **Reference** - Add to `reference/` for API docs and technical references
4. **Design** - Add to `design/` for architecture and design decisions

Update this README to include your new documentation!

---

## 📝 Documentation Conventions

- Use clear, concise language
- Include code examples with expected output
- Add troubleshooting sections
- Use consistent formatting (see existing docs)
- Include cross-references to related docs
- Add "Quick Copy-Paste" sections for common values

---

## 🔗 External Resources

- [Polkadot SDK Documentation](https://paritytech.github.io/polkadot-sdk/)
- [Substrate Documentation](https://docs.substrate.io/)
- [Polkadot.js Apps](https://polkadot.js.org/apps/)
- [Project Repository](https://github.com/paritytech/web3-storage)

---

**Last Updated:** February 2026
**Documentation Version:** 1.0
