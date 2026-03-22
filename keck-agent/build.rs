// SPDX-License-Identifier: Apache-2.0

fn main() {
    // Check if eBPF binary was pre-built (e.g., by Dockerfile)
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let prebuilt = format!("{}/ebpf", out_dir);

    if std::path::Path::new(&prebuilt).exists() {
        println!("cargo:rerun-if-changed={}", prebuilt);
        return; // Use pre-built eBPF binary
    }

    // Try building eBPF via aya-build
    let pkg = aya_build::Package {
        name: "keck-ebpf",
        root_dir: "../keck-ebpf",
        no_default_features: false,
        features: &[],
    };

    match aya_build::build_ebpf([pkg], aya_build::Toolchain::default()) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("eBPF build failed: {}. Agent will run without eBPF.", e);
            // Create empty placeholder so include_bytes! doesn't fail
            std::fs::write(&prebuilt, &[]).ok();
        }
    }
}
