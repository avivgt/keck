#!/bin/bash
# Install Keck Operator via OpenShift OperatorHub UI.
#
# Builds the operator and bundle images on the cluster,
# then creates a CatalogSource using the pre-built catalog from Quay.
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
echo "--- Step 1/3: Building operator image ---"
oc get bc keck-operator -n "$NAMESPACE" 2>/dev/null || \
  oc new-build --name=keck-operator --binary --strategy=docker -n "$NAMESPACE"
oc start-build keck-operator --from-dir="$OPERATOR_DIR" --follow -n "$NAMESPACE"

OPERATOR_IMAGE=$(oc get istag keck-operator:latest -n "$NAMESPACE" \
  -o jsonpath='{.image.dockerImageReference}')
echo "  Operator image: $OPERATOR_IMAGE"

# Step 2: Build the bundle image
echo
echo "--- Step 2/3: Building bundle image ---"
oc get bc keck-operator-bundle -n "$NAMESPACE" 2>/dev/null || \
  oc new-build --name=keck-operator-bundle --binary --strategy=docker -n "$NAMESPACE"

# Update the CSV with the operator image reference
TMPDIR=$(mktemp -d)
cp -r "$OPERATOR_DIR/bundle" "$TMPDIR/"
cp "$OPERATOR_DIR/bundle.Dockerfile" "$TMPDIR/Dockerfile"

# Replace the operator image in the CSV deployment
sed -i.bak "s|quay.io/aguetta/keck-operator:0.1.0|${OPERATOR_IMAGE}|g" \
  "$TMPDIR/bundle/manifests/keck-operator.clusterserviceversion.yaml"
sed -i.bak "s|namespace: placeholder|namespace: ${NAMESPACE}|g" \
  "$TMPDIR/bundle/manifests/keck-operator.clusterserviceversion.yaml"
rm -f "$TMPDIR/bundle/manifests/"*.bak

oc start-build keck-operator-bundle --from-dir="$TMPDIR" --follow -n "$NAMESPACE"
rm -rf "$TMPDIR"

BUNDLE_IMAGE=$(oc get istag keck-operator-bundle:latest -n "$NAMESPACE" \
  -o jsonpath='{.image.dockerImageReference}')
echo "  Bundle image: $BUNDLE_IMAGE"

# Step 3: Create CatalogSource (uses pre-built catalog from Quay)
echo
echo "--- Step 3/3: Creating CatalogSource ---"
cat <<YAML | oc apply -f -
apiVersion: operators.coreos.com/v1alpha1
kind: CatalogSource
metadata:
  name: keck-operator-catalog
  namespace: openshift-marketplace
spec:
  sourceType: grpc
  image: quay.io/aguetta/keck-catalog:latest
  displayName: Keck Power Management
  publisher: Keck Project
  updateStrategy:
    registryPoll:
      interval: 10m
YAML

echo
echo "Waiting for catalog to be ready..."
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
