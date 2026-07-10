# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Keck, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Instead, email the maintainers directly or use GitHub's private vulnerability reporting feature.

## Security Model

### Agent (keck-agent)

The agent runs as a DaemonSet with the following Linux capabilities:

- `CAP_SYS_ADMIN` -- required for eBPF program loading and tracepoint attachment
- `CAP_PERFMON` -- required for perf_event_open (hardware counter access)
- `CAP_BPF` -- required for BPF syscalls
- `CAP_SYS_RESOURCE` -- required for increasing BPF map limits

The agent reads `/proc` and `/sys` (read-only mounts) and `/sys/kernel/tracing` for eBPF tracepoint attachment.

The agent runs as root (UID 0) because eBPF tracepoint attachment requires it.

### Controller (keck-controller)

The controller runs as a non-root user (UID 65532) with read-only root filesystem and all capabilities dropped.

The report ingestion endpoint (`POST /api/v1/report`) requires bearer token authentication via the `KECK_API_KEY` environment variable.

Read endpoints (`GET /api/v1/*`) return cluster-wide power data and are not authenticated. Access is restricted by NetworkPolicy to authorized namespaces (keck-system, openshift-monitoring, openshift-console).

### Network Security

The operator creates a NetworkPolicy that restricts ingress to the controller:

- Agent pods can reach port 8080 (report ingestion)
- OpenShift monitoring can reach port 8080 (Prometheus scraping)
- OpenShift console can reach port 9443 (TLS proxy for UI)

### Supply Chain

- All container images are built from Red Hat UBI 9 base images
- The release script runs trivy CVE scanning and fails on critical vulnerabilities
- Image signing with cosign is supported (optional, requires key configuration)

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
