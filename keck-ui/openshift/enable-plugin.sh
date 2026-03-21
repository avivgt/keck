#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Enable the Keck Power Management plugin in the OpenShift console.
# Run this after deploying the plugin manifests.
#
# This patches the console operator to load our plugin.

set -euo pipefail

PLUGIN_NAME="keck-power-management"

echo "Enabling console plugin: $PLUGIN_NAME"

# Check if already enabled
CURRENT=$(oc get console.operator.openshift.io cluster \
  -o jsonpath='{.spec.plugins}' 2>/dev/null || echo "[]")

if echo "$CURRENT" | grep -q "$PLUGIN_NAME"; then
  echo "Plugin already enabled."
  exit 0
fi

# Patch the console operator to add our plugin
oc patch console.operator.openshift.io cluster \
  --type=json \
  --patch "[{\"op\": \"add\", \"path\": \"/spec/plugins/-\", \"value\": \"$PLUGIN_NAME\"}]"

echo "Plugin enabled. The console will reload automatically."
echo "Look for 'Power Management' in the left navigation."
