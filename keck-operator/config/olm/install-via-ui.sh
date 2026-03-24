#!/bin/bash
# Install Keck Operator via OpenShift OperatorHub UI.
#
# This script builds the operator bundle and catalog images on the cluster,
# then creates a CatalogSource so the operator appears in OperatorHub.
# After running this, go to:
#   Operators → OperatorHub → search "Keck" → Install
#
# Prerequisites:
#   - oc CLI logged in as cluster-admin
#   - keck-system namespace exists (oc new-project keck-system)

set -euo pipefail

NAMESPACE="keck-system"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OPERATOR_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "=== Building Keck Operator for OperatorHub ==="
echo "  Operator dir: $OPERATOR_DIR"
echo "  Namespace: $NAMESPACE"
echo

# Step 1: Build the operator image
echo "--- Step 1/5: Building operator image ---"
oc get bc keck-operator -n "$NAMESPACE" 2>/dev/null || \
  oc new-build --name=keck-operator --binary --strategy=docker -n "$NAMESPACE"
oc start-build keck-operator --from-dir="$OPERATOR_DIR" --follow -n "$NAMESPACE"

OPERATOR_IMAGE=$(oc get istag keck-operator:latest -n "$NAMESPACE" \
  -o jsonpath='{.image.dockerImageReference}')
echo "  Operator image: $OPERATOR_IMAGE"

# Step 2: Build the bundle image
echo
echo "--- Step 2/5: Building bundle image ---"
oc get bc keck-operator-bundle -n "$NAMESPACE" 2>/dev/null || \
  oc new-build --name=keck-operator-bundle --binary --strategy=docker -n "$NAMESPACE"

# Update the CSV with the operator image reference
TMPDIR=$(mktemp -d)
cp -r "$OPERATOR_DIR/bundle" "$TMPDIR/"
cp "$OPERATOR_DIR/bundle.Dockerfile" "$TMPDIR/Dockerfile"

# Replace the operator image in the CSV deployment
sed -i.bak "s|ghcr.io/avivgt/keck-operator:0.1.0|${OPERATOR_IMAGE}|g" \
  "$TMPDIR/bundle/manifests/keck-operator.clusterserviceversion.yaml"
sed -i.bak "s|namespace: placeholder|namespace: ${NAMESPACE}|g" \
  "$TMPDIR/bundle/manifests/keck-operator.clusterserviceversion.yaml"
rm -f "$TMPDIR/bundle/manifests/"*.bak

oc start-build keck-operator-bundle --from-dir="$TMPDIR" --follow -n "$NAMESPACE"
rm -rf "$TMPDIR"

BUNDLE_IMAGE=$(oc get istag keck-operator-bundle:latest -n "$NAMESPACE" \
  -o jsonpath='{.image.dockerImageReference}')
echo "  Bundle image: $BUNDLE_IMAGE"

# Step 3: Build the catalog image
echo
echo "--- Step 3/5: Building catalog image ---"
oc get bc keck-catalog -n "$NAMESPACE" 2>/dev/null || \
  oc new-build --name=keck-catalog --binary --strategy=docker -n "$NAMESPACE"

TMPDIR=$(mktemp -d)
mkdir -p "$TMPDIR/config/olm"
cp "$SCRIPT_DIR/catalog.Dockerfile" "$TMPDIR/Dockerfile"

# Create the FBC catalog with the real bundle image reference
cat > "$TMPDIR/config/olm/catalog.yaml" <<YAML
---
schema: olm.package
name: keck-operator
defaultChannel: alpha
---
schema: olm.channel
name: alpha
package: keck-operator
entries:
  - name: keck-operator.v0.1.0
---
schema: olm.bundle
name: keck-operator.v0.1.0
package: keck-operator
image: ${BUNDLE_IMAGE}
properties:
  - type: olm.package
    value:
      packageName: keck-operator
      version: 0.1.0
YAML

oc start-build keck-catalog --from-dir="$TMPDIR" --follow -n "$NAMESPACE"
rm -rf "$TMPDIR"

CATALOG_IMAGE=$(oc get istag keck-catalog:latest -n "$NAMESPACE" \
  -o jsonpath='{.image.dockerImageReference}')
echo "  Catalog image: $CATALOG_IMAGE"

# Step 4: Create CatalogSource
echo
echo "--- Step 4/5: Creating CatalogSource ---"
cat <<YAML | oc apply -f -
apiVersion: operators.coreos.com/v1alpha1
kind: CatalogSource
metadata:
  name: keck-operator-catalog
  namespace: openshift-marketplace
spec:
  sourceType: grpc
  image: ${CATALOG_IMAGE}
  displayName: Keck Power Management
  publisher: Keck Project
  updateStrategy:
    registryPoll:
      interval: 10m
YAML

# Step 5: Wait for catalog to be ready
echo
echo "--- Step 5/5: Waiting for catalog to be ready ---"
for i in $(seq 1 30); do
  STATE=$(oc get catalogsource keck-operator-catalog -n openshift-marketplace \
    -o jsonpath='{.status.connectionState.lastObservedState}' 2>/dev/null || echo "")
  if [ "$STATE" = "READY" ]; then
    echo "  Catalog is READY"
    break
  fi
  echo "  Waiting... ($i/30)"
  sleep 5
done

echo
echo "=== Done ==="
echo
echo "The Keck Operator is now available in OperatorHub."
echo "To install:"
echo "  1. Open the OpenShift console"
echo "  2. Go to Operators → OperatorHub"
echo "  3. Search for 'Keck'"
echo "  4. Click 'Keck Operator' → Install"
echo "  5. After installation, create a KeckCluster resource"
echo
