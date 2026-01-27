# CLI Compatibility Test Suite

This test suite uses the OpenFGA CLI (`fga`) to verify RSFGA's compatibility with OpenFGA.
Tests are written as `.fga.yaml` files and executed using `fga model test`.

## Prerequisites

1. Install the OpenFGA CLI:
   ```bash
   brew install openfga/tap/fga
   # or
   go install github.com/openfga/cli/cmd/fga@latest
   ```

2. Start RSFGA server:
   ```bash
   cargo run --release
   ```

## Running Tests

### Run all tests against RSFGA (default port 8080):
```bash
./scripts/run-tests.sh
```

### Run against a custom endpoint:
```bash
FGA_API_URL=http://localhost:8080 ./scripts/run-tests.sh
```

### Run a specific test file:
```bash
fga model test --tests tests/01-direct-relations.fga.yaml --api-url http://localhost:8080
```

### Compare RSFGA vs OpenFGA:
```bash
./scripts/compare-implementations.sh
```

## Test Categories

| File | Description |
|------|-------------|
| `01-direct-relations.fga.yaml` | Basic direct tuple assignments |
| `02-computed-unions.fga.yaml` | Union (OR) computed relations |
| `03-computed-intersections.fga.yaml` | Intersection (AND) computed relations |
| `04-computed-exclusions.fga.yaml` | Exclusion (but not) computed relations |
| `05-nested-relations.fga.yaml` | Multi-hop relation traversal |
| `06-wildcards.fga.yaml` | Wildcard (`*`) tuple assignments |
| `07-usersets.fga.yaml` | Userset references (`type#relation`) |
| `08-conditions.fga.yaml` | Conditional tuple evaluation |
| `09-list-objects.fga.yaml` | ListObjects API validation |
| `10-list-users.fga.yaml` | ListUsers API validation |
| `11-edge-cases.fga.yaml` | Boundary conditions and edge cases |
| `12-github-model.fga.yaml` | Real-world GitHub permissions model |
| `13-google-drive-model.fga.yaml` | Real-world Google Drive model |

## Test File Format

Each `.fga.yaml` file contains:

```yaml
name: Test Name
model: |
  model
    schema 1.1
  type user
  type document
    relations
      define viewer: [user]

tuples:
  - user: user:alice
    relation: viewer
    object: document:readme

tests:
  - name: "Direct relation check"
    check:
      - user: user:alice
        object: document:readme
        assertions:
          viewer: true
    list_objects:
      - user: user:alice
        type: document
        assertions:
          viewer:
            - document:readme
```

## Adding New Tests

1. Create a new `.fga.yaml` file in `tests/`
2. Define the authorization model
3. Add tuple fixtures
4. Write test assertions
5. Run `fga model test --tests your-test.fga.yaml` to verify
6. Add the test to the runner script
