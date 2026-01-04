---
name: block-implementation-without-approval
enabled: true
event: file
action: block
conditions:
  - field: file_path
    operator: regex_match
    pattern: (crates/rsfga-|src/.*\.rs$)
  - field: new_text
    operator: regex_match
    pattern: (pub\s+(async\s+)?fn|impl\s+\w+|struct\s+\w+|enum\s+\w+)
---

🚫 **Implementation Blocked - Approval Required**

**CLAUDE.md Project Rule**: "This is a design phase project - Do NOT implement without explicit approval"

**Current Phase**: Architecture & Design ✅ Complete | Implementation ⏸️ Awaiting Approval

**Why this is blocked**:
- You're attempting to create or modify Rust implementation files
- The project is currently in the design phase
- Implementation requires explicit user approval to begin

**Next Steps**:
1. **Get approval first**: Ask the user: "Should I begin implementation of Milestone X.X?"
2. **Wait for explicit "yes"** or "go ahead" confirmation
3. **Only then** proceed with creating/modifying implementation files

**Exceptions**:
- Test files in `crates/compatibility-tests/` (testing OpenFGA, not implementing RSFGA)
- Documentation updates (docs/, README.md, CLAUDE.md)
- Build configuration (Cargo.toml, .gitignore)

**To proceed**:
- Ask the user for permission to start implementation
- Get clear approval before writing implementation code
