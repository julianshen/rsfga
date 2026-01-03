# Contributing to RSFGA

Thank you for your interest in contributing to RSFGA! This document provides guidelines and workflows for contributing to the project.

## Table of Contents

- [Development Philosophy](#development-philosophy)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Code Quality Standards](#code-quality-standards)
- [Testing Requirements](#testing-requirements)
- [Commit Message Conventions](#commit-message-conventions)
- [Pull Request Process](#pull-request-process)
- [Code Review Guidelines](#code-review-guidelines)

## Development Philosophy

RSFGA follows **Test-Driven Development (TDD)** methodology as described in Kent Beck's approach:

1. **Red**: Write a failing test
2. **Green**: Write minimum code to make it pass
3. **Refactor**: Improve code while keeping tests green

### Critical Invariants

These must NEVER be violated:

- **I1: Correctness Over Performance** - Authorization bugs are security vulnerabilities
- **I2: 100% OpenFGA API Compatibility** - Drop-in replacement requirement
- **I3: Performance Claims Require Validation** - Benchmark before claiming improvements
- **I4: Security-Critical Code Protection** - Graph resolver requires rigorous testing

See [CLAUDE.md](CLAUDE.md) for detailed invariants and quality gates.

## Getting Started

### Prerequisites

- Rust 1.75+ ([Install Rust](https://rustup.rs/))
- Docker (for compatibility tests)
- Git

### Clone and Setup

```bash
# Clone repository
git clone https://github.com/your-org/rsfga.git
cd rsfga

# Install development tools
cargo install cargo-watch cargo-criterion

# Verify setup
cargo check
cargo test
cargo clippy
cargo fmt -- --check
```

### Understanding the Codebase

Before contributing, read these documents in order:

1. [README.md](README.md) - Project overview
2. [ARCHITECTURE.md](docs/design/ARCHITECTURE.md) - System architecture
3. [CLAUDE.md](CLAUDE.md) - Development methodology and TDD workflow
4. [plan.md](plan.md) - Current implementation tasks

## Development Workflow

### Branch Naming

```
feature/milestone-X.Y-description
```

Examples:
- `feature/milestone-0.1-test-harness`
- `feature/milestone-1.2-type-system`

### TDD Cycle

For each test in [plan.md](plan.md):

1. **Check plan.md** for next unchecked test `[ ]`
2. **Write failing test** (Red phase)
   ```bash
   cargo test --test <test_name>  # Should fail
   ```
3. **Write minimum code** to pass (Green phase)
   ```bash
   cargo test --test <test_name>  # Should pass
   ```
4. **Refactor** if needed (keep tests green)
5. **Run quality gates**:
   ```bash
   cargo test        # All tests pass
   cargo clippy      # Zero warnings
   cargo fmt         # Code formatted
   cargo audit       # No vulnerabilities
   ```
6. **Mark test complete** in plan.md: `[x]`
7. **Commit** with appropriate prefix

### Quality Gates (Must Pass Before Commit)

```bash
# Run all quality gates
cargo test && \
cargo clippy --all-targets --all-features && \
cargo fmt --all -- --check && \
cargo audit
```

## Code Quality Standards

### Rust Style

Follow official Rust style guide:

```rust
// ✅ GOOD: Clear, documented, idiomatic Rust
/// Check if user has permission on object.
///
/// # Arguments
/// * `user` - User identifier (e.g., "user:alice")
/// * `relation` - Relation to check (e.g., "viewer")
/// * `object` - Object identifier (e.g., "document:doc1")
///
/// # Returns
/// `Ok(true)` if permitted, `Ok(false)` otherwise.
#[instrument(skip(self))]
pub async fn check(
    &self,
    user: &str,
    relation: &str,
    object: &str,
) -> Result<bool> {
    // Implementation
}

// ❌ BAD: Undocumented, poor naming
pub fn chk(u: &str, r: &str, o: &str) -> bool {
    // Implementation
}
```

### Error Handling

```rust
// Use thiserror for domain errors
#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("depth limit exceeded (max: {max})")]
    DepthLimitExceeded { max: u32 },
}

// Use anyhow for application errors
pub type Result<T> = anyhow::Result<T>;
```

### Async Guidelines

```rust
// ✅ GOOD: Non-blocking async operations
async fn process_check(&self, check: Check) -> Result<bool> {
    let result = self.resolver.check(check).await?;
    Ok(result)
}

// ❌ BAD: Blocking operation in async context
async fn bad_async() {
    std::thread::sleep(Duration::from_secs(1)); // Blocks!
}
```

## Testing Requirements

### Test Pyramid

1. **Unit Tests** (90%+ coverage)
   - Fast, isolated component tests
   - File location: `#[cfg(test)] mod tests` in same file

2. **Integration Tests** (Component interactions)
   - File location: `tests/integration/`
   - Use testcontainers for databases

3. **Compatibility Tests** (Phase 0 validation)
   - File location: `tests/compatibility/`
   - Validates OpenFGA behavior parity

4. **Benchmarks** (Performance validation)
   - File location: `benches/`
   - Use criterion for benchmarking

### Writing Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_direct_relation_check() {
        // Arrange
        let resolver = setup_test_resolver().await;

        // Act
        let result = resolver.check(
            "user:alice",
            "viewer",
            "document:doc1"
        ).await;

        // Assert
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
```

## Commit Message Conventions

### Commit Prefixes

- `[BEHAVIORAL]` - Changes functionality/behavior
- `[STRUCTURAL]` - Refactoring without behavior change
- `[DOCS]` - Documentation updates
- `[TEST]` - Test additions/modifications
- `[ARCHITECTURAL]` - Major architectural changes

### Format

```
[PREFIX] Brief description (50 chars max)

Detailed explanation of what and why (if needed).
Wrap at 72 characters.

🎯 Generated with Claude Code
https://claude.com/claude-code

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>
```

### Examples

```bash
# Good commit messages
git commit -m "[BEHAVIORAL] Add direct relation check support"
git commit -m "[STRUCTURAL] Extract cache key generation to method"
git commit -m "[TEST] Add property tests for union relations"

# Bad commit messages
git commit -m "fix bug"
git commit -m "WIP"
git commit -m "update code"
```

## Pull Request Process

### PR Scope: Small and Reviewable

Create PRs **per section** (5-15 tests, <500 lines):

```
✅ Good:   Section 1 of Milestone 1.2 (10 tests, 300 lines)
⚠️  Too Large: Entire Milestone 1.2 (30 tests, 1200 lines)
```

### PR Workflow

1. **Complete a section** from plan.md
2. **Ensure quality gates pass**:
   ```bash
   cargo test && cargo clippy && cargo fmt && cargo audit
   ```
3. **Push feature branch**:
   ```bash
   git push origin feature/milestone-X.Y-description
   ```
4. **Create PR** with title format:
   ```
   [Milestone X.Y] Section N: Brief description
   ```
   Example: `[Milestone 0.1] Section 1: OpenFGA Test Environment`

5. **PR Description** must include:
   - Summary of changes
   - Completed tests from plan.md (marked `[x]`)
   - Why decisions (if architectural)
   - Test coverage metrics
   - Links to relevant ADRs/risks

6. **Wait for review** and approval
7. **Merge to main** (prefer squash merge)
8. **Continue on same branch** for next section
9. **Delete branch** only when milestone complete

### PR Template

```markdown
## Summary
Brief description of what this PR implements.

## Tests Completed
- [x] Test: Can start OpenFGA via Docker Compose
- [x] Test: Can connect to OpenFGA HTTP API
- [x] Test: Can connect to OpenFGA gRPC API

## Architectural Decisions
Reference any ADRs created or modified.

## Test Coverage
- Unit test coverage: 95%
- All quality gates passing

## Related Issues
Closes #123
```

## Code Review Guidelines

### For Reviewers

**Check for:**
- ✅ All tests pass in CI
- ✅ Code follows TDD methodology
- ✅ Invariants I1-I4 respected
- ✅ API compatibility maintained (if applicable)
- ✅ No blocking operations in async context
- ✅ Proper error handling
- ✅ Documentation for public APIs
- ✅ Test coverage >90%

**Don't:**
- ❌ Request "improvements" beyond task scope
- ❌ Nitpick formatting (cargo fmt handles this)
- ❌ Block on performance without benchmarks
- ❌ Suggest adding features not in plan.md

### For Contributors

**Before requesting review:**
- Run all quality gates locally
- Self-review your changes
- Verify all tests in PR description are marked `[x]` in plan.md
- Check no debug println!/dbg! left in code

## Development Commands

```bash
# Run tests
cargo test                    # All tests
cargo test --test <name>      # Specific test
cargo test -- --nocapture     # Show println! output

# Code quality
cargo clippy                  # Lint
cargo fmt                     # Format
cargo audit                   # Security vulnerabilities

# Benchmarks
cargo bench                   # Run all benchmarks
cargo bench --bench <name>    # Specific benchmark

# Watch mode (auto-run on changes)
cargo watch -x test
cargo watch -x clippy

# Coverage (requires tarpaulin)
cargo tarpaulin --out Html
```

## Getting Help

- **Questions**: [GitHub Discussions](https://github.com/your-org/rsfga/discussions)
- **Bugs**: [GitHub Issues](https://github.com/your-org/rsfga/issues)
- **Documentation**: See [docs/design/](docs/design/)
- **Methodology**: See [CLAUDE.md](CLAUDE.md)

## Areas for Contribution

### Phase 0 (Current)
- Compatibility test suite development
- Test data generators
- OpenFGA behavior documentation

### Phase 1 (Future)
- Core type system
- Graph resolver
- Storage backends
- API handlers

### General
- Documentation improvements
- Test coverage expansion
- Performance benchmarking
- Bug fixes

## License

By contributing to RSFGA, you agree that your contributions will be licensed under the Apache 2.0 License.

---

**Thank you for contributing to RSFGA!** 🎯

For detailed development methodology, see [CLAUDE.md](CLAUDE.md).
