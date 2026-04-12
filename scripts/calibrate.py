#!/usr/bin/env python3
"""
Keck alpha/beta coefficient calibration.

Runs controlled workloads on a target node, measures per-pod power via the
Keck controller API, and fits optimal alpha/beta coefficients via grid search.

Prerequisites:
  - oc/kubectl configured and logged in
  - Keck controller accessible (auto port-forward or --controller-url)
  - Calibration image built: quay.io/aguetta/keck-calibrate:latest

Usage:
  python3 scripts/calibrate.py --node worker1.example.com
  python3 scripts/calibrate.py --node worker1 --controller-url http://localhost:8080
"""

import argparse
import json
import subprocess
import sys
import time
from dataclasses import dataclass

NAMESPACE = "keck-calibrate"
DEFAULT_IMAGE = "quay.io/aguetta/keck-calibrate:latest"

# Workload profiles: (name, stress-ng args, expected IPC range, expected miss ratio range)
WORKLOADS = {
    "compute": {
        "args": ["stress-ng", "--matrix", "1", "--matrix-size", "128"],
        "description": "Compute-heavy (high IPC, low cache miss ratio)",
    },
    "memory": {
        "args": ["stress-ng", "--stream", "1", "--stream-l3-size", "32M"],
        "description": "Memory-heavy (low IPC, high cache miss ratio)",
    },
    "mixed": {
        "args": ["stress-ng", "--cpu", "1", "--vm", "1", "--vm-bytes", "128M"],
        "description": "Mixed workload",
    },
}


@dataclass
class WorkloadProfile:
    """Measured profile from an isolation run."""
    name: str
    avg_cpu_uw: float
    ipc: float
    miss_ratio: float


def run(cmd: list[str], check: bool = True, capture: bool = True) -> str:
    """Run a shell command and return stdout."""
    result = subprocess.run(cmd, capture_output=capture, text=True)
    if check and result.returncode != 0:
        print(f"Command failed: {' '.join(cmd)}", file=sys.stderr)
        if result.stderr:
            print(result.stderr, file=sys.stderr)
        sys.exit(1)
    return result.stdout.strip() if capture else ""


def kubectl(*args: str) -> str:
    """Run kubectl/oc command."""
    tool = "oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0 else "kubectl"
    return run([tool] + list(args))


def create_namespace():
    """Create the calibration namespace if it doesn't exist."""
    try:
        kubectl("get", "ns", NAMESPACE)
    except SystemExit:
        kubectl("create", "ns", NAMESPACE)
    print(f"Using namespace: {NAMESPACE}")


def cleanup():
    """Delete the calibration namespace."""
    print("\nCleaning up...")
    run(["oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0
         else "kubectl", "delete", "ns", NAMESPACE, "--ignore-not-found"], check=False)


def deploy_workload(name: str, node: str, image: str, args: list[str]) -> str:
    """Deploy a calibration workload pod. Returns pod name."""
    pod_name = f"keck-calib-{name}"
    manifest = {
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": pod_name,
            "namespace": NAMESPACE,
            "labels": {"app": "keck-calibrate", "workload": name},
        },
        "spec": {
            "nodeName": node,
            "containers": [{
                "name": "workload",
                "image": image,
                "command": args,
                "resources": {
                    "requests": {"cpu": "500m", "memory": "256Mi"},
                },
            }],
            "restartPolicy": "Never",
        },
    }

    kubectl("apply", "-f", "-", input_data=None)
    # Use subprocess directly to pipe stdin
    tool = "oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0 else "kubectl"
    proc = subprocess.run(
        [tool, "apply", "-f", "-"],
        input=json.dumps(manifest),
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        print(f"Failed to create pod {pod_name}: {proc.stderr}", file=sys.stderr)
        sys.exit(1)

    # Wait for running
    print(f"  Waiting for {pod_name} to start...", end="", flush=True)
    for _ in range(60):
        phase = kubectl("get", "pod", pod_name, "-n", NAMESPACE,
                        "-o", "jsonpath={.status.phase}")
        if phase == "Running":
            print(" running")
            return pod_name
        time.sleep(2)
        print(".", end="", flush=True)

    print(f"\n  Pod {pod_name} did not start in time", file=sys.stderr)
    sys.exit(1)


def delete_pod(name: str):
    """Delete a pod and wait for termination."""
    tool = "oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0 else "kubectl"
    run([tool, "delete", "pod", name, "-n", NAMESPACE, "--grace-period=0",
         "--force", "--ignore-not-found"], check=False)
    time.sleep(3)


def get_pod_power(controller_url: str, node: str, pod_name: str,
                  samples: int = 6, interval: int = 10) -> float:
    """Query Keck controller for per-pod CPU power, return average over samples."""
    import urllib.request

    powers = []
    url = f"{controller_url}/api/v1/pods-by-node?name={node}"

    for i in range(samples):
        if i > 0:
            time.sleep(interval)
        try:
            with urllib.request.urlopen(url, timeout=10) as resp:
                data = json.loads(resp.read())
        except Exception as e:
            print(f"  Warning: API query failed: {e}")
            continue

        for pod in data:
            if pod.get("pod_name", "").startswith(pod_name.split("-")[-1]) or \
               pod.get("pod_name") == pod_name:
                powers.append(pod.get("cpu_uw", 0))
                break

    if not powers:
        print(f"  Warning: No power data for {pod_name}")
        return 0.0

    avg = sum(powers) / len(powers)
    return avg


def get_perf_stats(pod_name: str) -> tuple[float, float]:
    """Run perf stat inside the pod to get IPC and cache miss ratio.
    Returns (ipc, cache_miss_ratio)."""
    tool = "oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0 else "kubectl"

    # Run perf stat for 10 seconds on the main workload PID
    try:
        output = run([
            tool, "exec", pod_name, "-n", NAMESPACE, "--",
            "perf", "stat", "-e", "instructions,cycles,cache-misses",
            "-a", "--", "sleep", "10",
        ])
    except SystemExit:
        print("  Warning: perf stat failed, using defaults")
        return (1.5, 0.01)

    instructions = 0
    cycles = 0
    misses = 0
    for line in output.split("\n"):
        line = line.strip().replace(",", "")
        parts = line.split()
        if len(parts) >= 2:
            try:
                val = int(parts[0])
            except ValueError:
                continue
            if "instructions" in line:
                instructions = val
            elif "cycles" in line:
                cycles = val
            elif "cache-misses" in line:
                misses = val

    ipc = instructions / cycles if cycles > 0 else 1.5
    miss_ratio = misses / instructions if instructions > 0 else 0.01

    return (ipc, miss_ratio)


def predict_ratio(profile_a: WorkloadProfile, profile_b: WorkloadProfile,
                  alpha: float, beta: float) -> float:
    """Predict the power ratio A/B under given alpha, beta.

    Assumes equal time (both workloads running simultaneously).
    """
    weight_a = 1.0 + alpha * profile_a.ipc + beta * profile_a.miss_ratio
    weight_b = 1.0 + alpha * profile_b.ipc + beta * profile_b.miss_ratio
    if weight_b <= 0:
        return float("inf")
    return weight_a / weight_b


def fit_coefficients(profiles: list[WorkloadProfile],
                     alpha_range: tuple[float, float, float],
                     beta_range: tuple[float, float, float]) -> tuple[float, float, float]:
    """Grid search for optimal alpha, beta.

    Returns (best_alpha, best_beta, best_error).
    """
    import itertools

    # Ground truth ratios from isolation runs
    pairs = []
    for i in range(len(profiles)):
        for j in range(i + 1, len(profiles)):
            if profiles[j].avg_cpu_uw > 0:
                gt_ratio = profiles[i].avg_cpu_uw / profiles[j].avg_cpu_uw
                pairs.append((profiles[i], profiles[j], gt_ratio))

    if not pairs:
        print("Error: not enough valid profiles for fitting", file=sys.stderr)
        return (0.3, 1.5, float("inf"))

    best_alpha, best_beta, best_error = 0.3, 1.5, float("inf")

    a_min, a_max, a_step = alpha_range
    b_min, b_max, b_step = beta_range

    a = a_min
    while a <= a_max:
        b = b_min
        while b <= b_max:
            error = 0.0
            for pa, pb, gt in pairs:
                pred = predict_ratio(pa, pb, a, b)
                error += (pred - gt) ** 2
            if error < best_error:
                best_error = error
                best_alpha = a
                best_beta = b
            b += b_step
        a += a_step

    return (best_alpha, best_beta, best_error)


def main():
    parser = argparse.ArgumentParser(description="Keck alpha/beta coefficient calibration")
    parser.add_argument("--node", required=True, help="Target node name")
    parser.add_argument("--controller-url", default=None,
                        help="Keck controller URL (default: auto port-forward)")
    parser.add_argument("--image", default=DEFAULT_IMAGE, help="Calibration workload image")
    parser.add_argument("--stabilize-secs", type=int, default=30,
                        help="Wait time for workload stability")
    parser.add_argument("--measure-secs", type=int, default=60,
                        help="Measurement window (seconds)")
    parser.add_argument("--alpha-range", default="0.0,2.0,0.05",
                        help="Alpha search range: min,max,step")
    parser.add_argument("--beta-range", default="0.0,5.0,0.1",
                        help="Beta search range: min,max,step")
    args = parser.parse_args()

    alpha_range = tuple(float(x) for x in args.alpha_range.split(","))
    beta_range = tuple(float(x) for x in args.beta_range.split(","))

    # Setup controller URL
    controller_url = args.controller_url
    if not controller_url:
        print("No --controller-url specified. Attempting port-forward...")
        controller_url = "http://localhost:8080"
        # Start port-forward in background
        tool = "oc" if subprocess.run(["which", "oc"], capture_output=True).returncode == 0 else "kubectl"
        pf = subprocess.Popen(
            [tool, "port-forward", "-n", "keck-system", "svc/keck-controller", "8080:8080"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        time.sleep(3)
    else:
        pf = None

    try:
        print("=" * 60)
        print("Keck Coefficient Calibration")
        print(f"  Node: {args.node}")
        print(f"  Controller: {controller_url}")
        print(f"  Image: {args.image}")
        print("=" * 60)

        create_namespace()

        # Phase 1: Isolation runs
        print("\n--- Phase 1: Isolation runs ---")
        profiles: list[WorkloadProfile] = []

        for wl_name, wl_config in WORKLOADS.items():
            print(f"\n  [{wl_name}] {wl_config['description']}")
            pod_name = deploy_workload(wl_name, args.node, args.image, wl_config["args"])

            print(f"  Stabilizing ({args.stabilize_secs}s)...")
            time.sleep(args.stabilize_secs)

            print(f"  Measuring power ({args.measure_secs}s)...")
            samples = max(1, args.measure_secs // 10)
            avg_power = get_pod_power(controller_url, args.node, pod_name, samples=samples)
            print(f"    Average CPU power: {avg_power / 1e6:.2f} W")

            print(f"  Collecting perf stats...")
            ipc, miss_ratio = get_perf_stats(pod_name)
            print(f"    IPC: {ipc:.3f}, cache miss ratio: {miss_ratio:.6f}")

            profiles.append(WorkloadProfile(
                name=wl_name,
                avg_cpu_uw=avg_power,
                ipc=ipc,
                miss_ratio=miss_ratio,
            ))

            delete_pod(pod_name)

        # Phase 2: Fitting
        print("\n--- Phase 2: Grid search fitting ---")
        print(f"  Alpha range: {alpha_range}")
        print(f"  Beta range: {beta_range}")

        # Ground truth ratios
        print("\n  Ground truth ratios (from isolation runs):")
        for i in range(len(profiles)):
            for j in range(i + 1, len(profiles)):
                if profiles[j].avg_cpu_uw > 0:
                    ratio = profiles[i].avg_cpu_uw / profiles[j].avg_cpu_uw
                    print(f"    {profiles[i].name}/{profiles[j].name} = {ratio:.3f}")

        best_alpha, best_beta, best_error = fit_coefficients(profiles, alpha_range, beta_range)

        print(f"\n  Predicted ratios with default (alpha=0.3, beta=1.5):")
        for i in range(len(profiles)):
            for j in range(i + 1, len(profiles)):
                pred = predict_ratio(profiles[i], profiles[j], 0.3, 1.5)
                gt = profiles[i].avg_cpu_uw / profiles[j].avg_cpu_uw if profiles[j].avg_cpu_uw > 0 else 0
                print(f"    {profiles[i].name}/{profiles[j].name} = {pred:.3f} (ground truth: {gt:.3f})")

        print(f"\n  Predicted ratios with optimal (alpha={best_alpha:.2f}, beta={best_beta:.2f}):")
        for i in range(len(profiles)):
            for j in range(i + 1, len(profiles)):
                pred = predict_ratio(profiles[i], profiles[j], best_alpha, best_beta)
                gt = profiles[i].avg_cpu_uw / profiles[j].avg_cpu_uw if profiles[j].avg_cpu_uw > 0 else 0
                print(f"    {profiles[i].name}/{profiles[j].name} = {pred:.3f} (ground truth: {gt:.3f})")

        # Results
        print("\n" + "=" * 60)
        print("RESULTS")
        print("=" * 60)
        print(f"  Optimal alpha = {best_alpha:.3f}")
        print(f"  Optimal beta  = {best_beta:.3f}")
        print(f"  Fitting error = {best_error:.6f}")
        print()
        print("  To apply, set in the agent DaemonSet environment:")
        print(f'    KECK_ALPHA="{best_alpha:.3f}"')
        print(f'    KECK_BETA="{best_beta:.3f}"')
        print()
        print("  Or patch the KeckCluster:")
        print(f"    oc set env daemonset/keck-agent -n keck-system \\")
        print(f'      KECK_ALPHA="{best_alpha:.3f}" KECK_BETA="{best_beta:.3f}"')
        print()

    finally:
        cleanup()
        if pf:
            pf.terminate()


if __name__ == "__main__":
    main()
