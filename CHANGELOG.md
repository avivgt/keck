# Changelog

All notable changes to Keck will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Liveness and readiness probes for agent and controller pods
- Prometheus /metrics endpoint on the controller with cluster/namespace/node power gauges
- PrometheusRule with KeckControllerDown, KeckAgentNotReporting, KeckHighErrorRatio, KeckNoPowerData alerts
- ServiceMonitor auto-created by operator for controller metrics scraping
- Kubernetes Events emitted by operator on reconcile success/failure
- NetworkPolicy restricting ingress to the controller
- GitHub Actions CI workflow (Go lint+test, Rust check+test)
- CHANGELOG.md, CONTRIBUTING.md, SECURITY.md
- PodDisruptionBudget for controller
- Separate ServiceAccount for controller (no longer shares keck-agent SA)

### Changed
- Agent DaemonSet uses explicit capabilities (SYS_ADMIN, PERFMON, BPF, SYS_RESOURCE) instead of privileged: true
- Nginx TLS proxy pinned to nginxinc/nginx-unprivileged:1.27-alpine (was floating :alpine tag)
- Controller and nginx containers have securityContext with readOnlyRootFilesystem and drop ALL capabilities
- RBAC reconciliation is now idempotent (create-or-update, not create-only)
- OLM CSV includes olm.skipRange for upgrade path
- Agent Dockerfile explicitly sets USER 0
- Release script includes trivy CVE scanning and optional cosign signing
- README rewritten to separate working features from roadmap

### Fixed
- PMC_ENABLED eBPF map changed from PerCpuArray to Array (hardware counters were only active on CPU 0)
- DeepCopy corruption on RedfishSpec.NodeBMCMap and AgentSpec.CapturedLabels
- Cross-scope Owns() replaced with Watches()+EnqueueRequestsFromMapFunc for cluster-scoped CRD owning namespaced resources
- DefaultProfile, GPUEnabled, CapturedLabels spec fields now injected as agent env vars (were defined but ignored)
- Three operator test assertions updated to match current code (volume mount count, service port count, default image repo)
- Agent K8s client creation uses retry with backoff instead of expect() panic

## [0.1.0] - 2026-03-26

### Added
- Initial release
- Per-workload power attribution with eBPF (sched_switch, cpu_frequency, hardware counters)
- RAPL, hwmon, Redfish, DCGM hardware source discovery
- Three attribution models: FullModel, FrequencyWeighted, CpuTimeRatio
- Cluster controller with REST API and aggregation
- OpenShift console plugin UI
- KeckCluster CRD with OLM operator
- Kepler side-by-side comparison
