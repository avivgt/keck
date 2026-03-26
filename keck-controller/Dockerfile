# Build stage: Rust stable (no eBPF needed for controller)
FROM registry.access.redhat.com/ubi9/ubi:latest AS builder

# Install Rust
RUN dnf install -y gcc gcc-c++ make curl && dnf clean all
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

# Copy only what the controller needs
COPY Cargo.toml ./
COPY keck-common/ keck-common/
COPY keck-controller/ keck-controller/

# Create minimal workspace Cargo.toml for controller only
RUN echo '[workspace]\nresolver = "2"\nmembers = ["keck-common", "keck-controller"]\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\nlicense = "Apache-2.0"\n\n[workspace.dependencies]\nthiserror = "2"\nlog = "0.4"' > Cargo.toml

# Build
RUN cargo build --release -p keck-controller

# Runtime stage
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest

LABEL name="keck-controller" \
      vendor="Keck Project" \
      version="0.1.0" \
      summary="Keck cluster controller" \
      description="Aggregates power data, carbon tracking, power-aware scheduling" \
      io.k8s.display-name="Keck Controller" \
      io.openshift.tags="monitoring,power,energy,carbon"

COPY --from=builder /build/target/release/keck-controller /usr/bin/keck-controller

USER 65532:65532

ENTRYPOINT ["/usr/bin/keck-controller"]
