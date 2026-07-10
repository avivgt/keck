# Contributing to Keck

## Development Setup

### Prerequisites

- Go 1.22+ (operator)
- Rust nightly (agent, controller)
- Linux (eBPF programs target the Linux kernel)
- OpenShift 4.14+ or Kubernetes 1.28+ cluster for testing

### Building

```bash
# Rust components
cargo build -p keck-agent
cargo build -p keck-controller

# Go operator
cd keck-operator && go build -o bin/manager cmd/main.go
```

### Testing

```bash
# All Rust tests
cargo test --workspace

# Go operator tests
cd keck-operator && go test ./... -v

# Single Rust crate
cargo test -p keck-controller
```

## Making Changes

1. Create a branch from `main`
2. Make your changes
3. Run tests locally
4. Submit a pull request

### Code Style

- Rust: follow `cargo clippy` recommendations
- Go: follow `golangci-lint` recommendations
- No comments explaining what code does (names should be self-documenting)
- Comments only for non-obvious constraints or workarounds

### Commit Messages

Use conventional commit format:

```
fix: correct PMC_ENABLED map type for multi-CPU hardware counters
feat: add Prometheus /metrics endpoint to controller
docs: update README to reflect actual feature status
```

## Architecture

See the README for the full architecture overview. Key directories:

- `keck-agent/src/` -- Rust node agent (eBPF, hardware, attribution)
- `keck-controller/src/` -- Rust cluster controller (aggregation, API)
- `keck-operator/` -- Go operator (CRDs, reconciliation)
- `keck-ui/` -- TypeScript OpenShift console plugin
- `keck-ebpf/` -- Rust no_std eBPF programs
- `keck-common/` -- Shared types (no_std compatible)

## Reporting Issues

Use GitHub Issues. Include:

- Keck version (or commit hash)
- OpenShift/Kubernetes version
- Hardware (CPU model, BMC type if using Redfish)
- Steps to reproduce
- Relevant logs (`oc logs ds/keck-agent -n keck-system`)
