// SPDX-License-Identifier: Apache-2.0

//! Build script: compiles the eBPF programs from ../keck-ebpf/ into the agent binary.
//! Uses aya-build to handle the BPF target cross-compilation.

fn main() {
    aya_build::build_ebpf(["../keck-ebpf"]).expect("failed to build eBPF programs");
}
