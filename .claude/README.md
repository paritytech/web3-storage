# Claude Code Skills and Commands

This directory contains skills and commands for Claude Code to assist with development tasks.

## Structure

```
.claude/
├── commands/           # User-invocable commands
│   └── review-pr.md   # Review pull requests
└── skills/            # Specialized task skills
    └── test/
        └── pallet.md  # Test the storage pallet
```

## Commands

Commands can be invoked by users via slash commands (e.g., `/review-pr 123`).

### `/review-pr <PR-NUMBER>`

Reviews a pull request against Substrate/Polkadot SDK best practices:
- Code quality (Rust idioms, error handling)
- Security (unsafe blocks, input validation, bounded collections)
- Performance (weights, storage patterns, arithmetic)
- Testing (coverage, edge cases)
- Breaking changes (API compatibility)
- FRAME pallet standards (storage, events, errors, weights)

**Usage**: `/review-pr 42`

## Skills

Skills are specialized agents that can be invoked to perform specific tasks.

### `test-pallet`

Runs comprehensive tests for the storage pallet including:
- Format and lint checks
- Unit tests (pallet, primitives, runtime)
- Integration tests (provider node, client SDK)
- Benchmark tests (optional)
- Coverage reports (optional)

**Flags**:
- `--benchmarks` - Include benchmark tests
- `--coverage` - Generate coverage report

## Adding New Skills

1. Create a new `.md` file in the appropriate subdirectory
2. For user-invocable skills, use the frontmatter format:

```markdown
---
name: skill-name
description: Brief description
argument-hint: "<arg1> [--flag]"
disable-model-invocation: true
user-invocable: true
allowed-tools: Bash, Read, Edit, Glob, Grep
---

# Skill content here
```

3. Update this README with the new skill documentation

## Code Review Guidelines

All contributions should follow the guidelines in [CLAUDE.md](../CLAUDE.md), which include:

- **Rust Code Quality**: Result types, arithmetic safety, clear naming
- **FRAME Standards**: Proper storage types, events, errors, weights
- **Security**: No panics, bounded collections, input validation
- **Testing**: Unit tests, edge cases, integration tests

## Resources

- [CLAUDE.md](../CLAUDE.md) - Complete project documentation for Claude Code
- [Parity Standards](https://github.com/paritytech/polkadot-sdk/blob/master/CLAUDE.md) - Upstream guidelines
- [Documentation Index](../docs/README.md) - All project documentation
