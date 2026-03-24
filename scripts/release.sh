#!/bin/bash
# Build and push all Keck images to ghcr.io for public consumption.
#
# Prerequisites:
#   - podman or docker logged in to ghcr.io
#   - opm CLI installed (for catalog build)
#
# Usage:
#   ./scripts/release.sh [VERSION]
#   ./scripts/release.sh 0.1.0

set -euo pipefail

VERSION="${1:-0.1.0}"
REGISTRY="ghcr.io/avivgt"
CONTAINER_TOOL="${CONTAINER_TOOL:-podman}"

OPERATOR_IMG="${REGISTRY}/keck-operator:v${VERSION}"
BUNDLE_IMG="${REGISTRY}/keck-operator-bundle:v${VERSION}"
CATALOG_IMG="${REGISTRY}/keck-catalog:v${VERSION}"
CATALOG_LATEST="${REGISTRY}/keck-catalog:latest"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== Keck Release v${VERSION} ==="
echo "  Registry: ${REGISTRY}"
echo "  Tool: ${CONTAINER_TOOL}"
echo

# Step 1: Build and push operator image
echo "--- 1/4: Operator image ---"
cd "$ROOT_DIR/keck-operator"
${CONTAINER_TOOL} build -t "${OPERATOR_IMG}" .
${CONTAINER_TOOL} push "${OPERATOR_IMG}"
echo "  Pushed: ${OPERATOR_IMG}"

# Step 2: Build and push bundle image
echo
echo "--- 2/4: Bundle image ---"
# Update CSV with the operator image
TMPDIR=$(mktemp -d)
cp -r bundle "$TMPDIR/"
cp bundle.Dockerfile "$TMPDIR/Dockerfile"
sed -i "s|ghcr.io/avivgt/keck-operator:0.1.0|${OPERATOR_IMG}|g" \
  "$TMPDIR/bundle/manifests/keck-operator.clusterserviceversion.yaml"

cd "$TMPDIR"
${CONTAINER_TOOL} build -t "${BUNDLE_IMG}" .
${CONTAINER_TOOL} push "${BUNDLE_IMG}"
rm -rf "$TMPDIR"
echo "  Pushed: ${BUNDLE_IMG}"

# Step 3: Build and push catalog image
echo
echo "--- 3/4: Catalog image ---"
TMPDIR=$(mktemp -d)
mkdir -p "$TMPDIR/configs/keck-operator"

cat > "$TMPDIR/configs/keck-operator/catalog.yaml" <<YAML
---
schema: olm.package
name: keck-operator
defaultChannel: alpha
---
schema: olm.channel
name: alpha
package: keck-operator
entries:
  - name: keck-operator.v${VERSION}
---
schema: olm.bundle
name: keck-operator.v${VERSION}
package: keck-operator
image: ${BUNDLE_IMG}
properties:
  - type: olm.package
    value:
      packageName: keck-operator
      version: ${VERSION}
YAML

cat > "$TMPDIR/Dockerfile" <<DOCKER
FROM registry.redhat.io/openshift4/ose-operator-registry:v4.14 AS builder
COPY configs /configs
RUN ["/bin/opm", "serve", "/configs", "--cache-dir=/tmp/cache", "--cache-only"]

FROM registry.redhat.io/openshift4/ose-operator-registry:v4.14
COPY --from=builder /configs /configs
COPY --from=builder /tmp/cache /tmp/cache
EXPOSE 50051
ENTRYPOINT ["/bin/opm"]
CMD ["serve", "/configs", "--cache-dir=/tmp/cache"]
LABEL operators.operatorframework.io.index.configs.v1=/configs
DOCKER

cd "$TMPDIR"
${CONTAINER_TOOL} build -t "${CATALOG_IMG}" .
${CONTAINER_TOOL} tag "${CATALOG_IMG}" "${CATALOG_LATEST}"
${CONTAINER_TOOL} push "${CATALOG_IMG}"
${CONTAINER_TOOL} push "${CATALOG_LATEST}"
rm -rf "$TMPDIR"
echo "  Pushed: ${CATALOG_IMG}"
echo "  Pushed: ${CATALOG_LATEST}"

# Step 4: Verify
echo
echo "--- 4/4: Verify ---"
echo "  Operator:  ${OPERATOR_IMG}"
echo "  Bundle:    ${BUNDLE_IMG}"
echo "  Catalog:   ${CATALOG_IMG}"
echo "  Catalog:   ${CATALOG_LATEST}"
echo
echo "=== Release complete ==="
echo
echo "Users can now install on OpenShift:"
echo "  oc apply -f https://raw.githubusercontent.com/avivgt/keck/main/install.yaml"
echo "  Then: Operators → OperatorHub → search 'Keck' → Install"
echo
