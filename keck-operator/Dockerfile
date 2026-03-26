# Build stage — native arch, cross-compile Go for target
FROM registry.access.redhat.com/ubi9/go-toolset:1.24 AS builder
USER 0

WORKDIR /workspace
COPY go.mod go.sum ./
RUN go mod download

COPY cmd/ cmd/
COPY api/ api/
COPY controllers/ controllers/

ARG TARGETARCH=amd64
RUN CGO_ENABLED=0 GOOS=linux GOARCH=${TARGETARCH} \
    go build -a -o manager cmd/main.go

# Runtime stage — Red Hat UBI minimal (standard for OpenShift operators)
FROM --platform=linux/amd64 registry.access.redhat.com/ubi9/ubi-minimal:latest

LABEL name="keck-operator" \
      vendor="Keck Project" \
      version="0.1.0" \
      summary="Keck Operator — Kubernetes power metering lifecycle management" \
      description="Manages Keck agent, controller, and fleet components on Kubernetes and OpenShift" \
      io.k8s.display-name="Keck Operator" \
      io.k8s.description="Kubernetes operator for deploying and managing Keck power metering" \
      io.openshift.tags="monitoring,power,energy,sustainability" \
      com.redhat.component="keck-operator-container"

WORKDIR /
COPY --from=builder /workspace/manager .

# Red Hat requires non-root
USER 65532:65532

ENTRYPOINT ["/manager"]
