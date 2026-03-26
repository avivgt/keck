# Build stage: Rust nightly with eBPF toolchain
FROM rust:latest AS builder

# Install nightly toolchain and BPF target
RUN rustup toolchain install nightly --component rust-src
RUN rustup default nightly

# Install build dependencies for eBPF
RUN apt-get update && apt-get install -y \
    clang llvm libelf-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install bpf-linker

WORKDIR /build

# Copy only what the agent needs
COPY Cargo.toml ./
COPY keck-common/ keck-common/
COPY keck-ebpf/ keck-ebpf/
COPY keck-agent/ keck-agent/

# Create minimal workspace Cargo.toml for agent only
RUN echo '[workspace]\nresolver = "2"\nmembers = ["keck-common", "keck-ebpf", "keck-agent"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\nlicense = "Apache-2.0"\n\n[workspace.dependencies]\nthiserror = "2"\nlog = "0.4"' > Cargo.toml

# Build the agent (includes eBPF compilation via aya-build)
RUN cargo build --release -p keck-agent

# Runtime stage
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest

LABEL name="keck-agent" \
      vendor="Keck Project" \
      version="0.1.0" \
      summary="Keck power metering node agent" \
      description="Per-node agent with eBPF-based power attribution" \
      io.k8s.display-name="Keck Agent" \
      io.openshift.tags="monitoring,power,energy,ebpf"

COPY --from=builder /build/target/release/keck-agent /usr/bin/keck-agent

ENTRYPOINT ["/usr/bin/keck-agent"]
