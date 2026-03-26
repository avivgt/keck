# Build stage: Rust nightly with eBPF toolchain
FROM registry.access.redhat.com/ubi9/ubi:latest AS builder

# Install Rust, nightly toolchain, and eBPF build dependencies
RUN dnf install -y gcc gcc-c++ make clang llvm elfutils-libelf-devel && \
    dnf clean all
RUN curl-minimal --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup component add rust-src --toolchain nightly
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
