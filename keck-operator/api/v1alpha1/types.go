// SPDX-License-Identifier: Apache-2.0

package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// ─── KeckCluster ─────────────────────────────────────────────────
// Top-level CRD: "I want Keck running in this cluster."
// The operator reconciles this into a DaemonSet (agent), Deployment
// (controller), RBAC, Services, and ServiceMonitor.

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Cluster
// +kubebuilder:printcolumn:name="Agents",type=integer,JSONPath=`.status.agentReady`
// +kubebuilder:printcolumn:name="Controller",type=string,JSONPath=`.status.controllerReady`
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
type KeckCluster struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   KeckClusterSpec   `json:"spec,omitempty"`
	Status KeckClusterStatus `json:"status,omitempty"`
}

type KeckClusterSpec struct {
	// Agent configuration (DaemonSet)
	Agent AgentSpec `json:"agent,omitempty"`

	// Controller configuration (Deployment)
	Controller ControllerSpec `json:"controller,omitempty"`

	// Fleet manager endpoint (optional — if set, controller reports to fleet)
	FleetEndpoint string `json:"fleetEndpoint,omitempty"`

	// Image settings
	Image ImageSpec `json:"image,omitempty"`
}

type AgentSpec struct {
	// Default profile for all nodes: "minimal", "standard", "full"
	// +kubebuilder:default=standard
	// +kubebuilder:validation:Enum=minimal;standard;full
	DefaultProfile string `json:"defaultProfile,omitempty"`

	// Node selector for agent DaemonSet (optional — default: all nodes)
	NodeSelector map[string]string `json:"nodeSelector,omitempty"`

	// Tolerations for agent DaemonSet
	Tolerations []corev1.Toleration `json:"tolerations,omitempty"`

	// Resource limits for agent pods
	Resources corev1.ResourceRequirements `json:"resources,omitempty"`

	// Enable GPU power monitoring
	// +kubebuilder:default=false
	GPUEnabled bool `json:"gpuEnabled,omitempty"`

	// Redfish BMC configuration (optional)
	Redfish *RedfishSpec `json:"redfish,omitempty"`
}

type RedfishSpec struct {
	// CredentialsSecret references a Secret with keys "username" and "password"
	// for BMC authentication. The Secret must exist in the keck-system namespace.
	CredentialsSecret string `json:"credentialsSecret"`

	// NodeBMCMap maps node serial numbers to BMC/iDRAC endpoint URLs.
	// The agent reads the node's serial from /sys/class/dmi/id/product_serial
	// and looks up its BMC endpoint from this map.
	//
	// Example:
	//   - serial: "41DQMH3"
	//     endpoint: "https://192.168.52.166"
	//   - serial: "D59N3L3"
	//     endpoint: "https://192.168.52.172"
	NodeBMCMap []NodeBMCEntry `json:"nodeBMCMap"`
}

type NodeBMCEntry struct {
	// Serial number of the node (from DMI/SMBIOS product_serial)
	Serial string `json:"serial"`
	// BMC/iDRAC endpoint URL (e.g., https://192.168.52.172)
	Endpoint string `json:"endpoint"`
}

type ControllerSpec struct {
	// Number of controller replicas (1 for most clusters, 2 for HA)
	// +kubebuilder:default=1
	// +kubebuilder:validation:Minimum=1
	// +kubebuilder:validation:Maximum=3
	Replicas int32 `json:"replicas,omitempty"`

	// Resource limits for controller pod
	Resources corev1.ResourceRequirements `json:"resources,omitempty"`

	// Enable power-aware scheduler extender
	// +kubebuilder:default=false
	SchedulerEnabled bool `json:"schedulerEnabled,omitempty"`

	// Carbon intensity API endpoint (optional)
	CarbonAPIEndpoint string `json:"carbonAPIEndpoint,omitempty"`

	// Carbon region identifier (e.g., "US-CAL-CISO")
	CarbonRegion string `json:"carbonRegion,omitempty"`

	// Energy cost per kWh
	// +kubebuilder:default="0.10"
	EnergyCostPerKWh string `json:"energyCostPerKWh,omitempty"`
}

type ImageSpec struct {
	// Container image repository
	// +kubebuilder:default="ghcr.io/avivgt/keck"
	Repository string `json:"repository,omitempty"`

	// Image tag
	// +kubebuilder:default="latest"
	Tag string `json:"tag,omitempty"`

	// Image pull policy
	// +kubebuilder:default=IfNotPresent
	PullPolicy corev1.PullPolicy `json:"pullPolicy,omitempty"`

	// Image pull secrets
	PullSecrets []corev1.LocalObjectReference `json:"pullSecrets,omitempty"`
}

type KeckClusterStatus struct {
	// Current phase: Pending, Installing, Running, Error
	Phase string `json:"phase,omitempty"`

	// Number of agent pods ready
	AgentReady int32 `json:"agentReady,omitempty"`

	// Number of agent pods desired
	AgentDesired int32 `json:"agentDesired,omitempty"`

	// Whether the controller is ready
	ControllerReady bool `json:"controllerReady,omitempty"`

	// Last time the status was updated
	LastUpdated metav1.Time `json:"lastUpdated,omitempty"`

	// Human-readable message
	Message string `json:"message,omitempty"`

	// Conditions
	Conditions []metav1.Condition `json:"conditions,omitempty"`
}

// +kubebuilder:object:root=true
type KeckClusterList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []KeckCluster `json:"items"`
}

// ─── PowerBudget ─────────────────────────────────────────────────
// Per-namespace power budget. When set, the scheduler extender
// rejects pods that would cause the namespace to exceed its budget.

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:scope=Namespaced
// +kubebuilder:printcolumn:name="Budget (W)",type=string,JSONPath=`.spec.maxWatts`
// +kubebuilder:printcolumn:name="Current (W)",type=string,JSONPath=`.status.currentWatts`
// +kubebuilder:printcolumn:name="Usage",type=string,JSONPath=`.status.usagePercent`
type PowerBudget struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   PowerBudgetSpec   `json:"spec,omitempty"`
	Status PowerBudgetStatus `json:"status,omitempty"`
}

type PowerBudgetSpec struct {
	// Maximum power in watts for this namespace
	// +kubebuilder:validation:Minimum=0
	MaxWatts float64 `json:"maxWatts"`

	// Action when budget is exceeded: "alert", "throttle", "reject"
	// +kubebuilder:default=alert
	// +kubebuilder:validation:Enum=alert;throttle;reject
	Action string `json:"action,omitempty"`

	// Alert webhook URL (optional, for action=alert)
	AlertWebhook string `json:"alertWebhook,omitempty"`
}

type PowerBudgetStatus struct {
	// Current power usage in watts
	CurrentWatts float64 `json:"currentWatts,omitempty"`

	// Usage as percentage of budget
	UsagePercent string `json:"usagePercent,omitempty"`

	// Whether the budget is currently exceeded
	Exceeded bool `json:"exceeded,omitempty"`

	// Last time the status was updated
	LastUpdated metav1.Time `json:"lastUpdated,omitempty"`
}

// +kubebuilder:object:root=true
type PowerBudgetList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []PowerBudget `json:"items"`
}

// ─── PowerProfile ────────────────────────────────────────────────
// Override agent profile for specific nodes (by label selector).
// E.g., "use full profile on GPU nodes, minimal on edge nodes."

// +kubebuilder:object:root=true
// +kubebuilder:resource:scope=Cluster
// +kubebuilder:printcolumn:name="Profile",type=string,JSONPath=`.spec.profile`
// +kubebuilder:printcolumn:name="Nodes",type=integer,JSONPath=`.status.matchingNodes`
type PowerProfile struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   PowerProfileSpec   `json:"spec,omitempty"`
	Status PowerProfileStatus `json:"status,omitempty"`
}

type PowerProfileSpec struct {
	// Agent profile to apply: "minimal", "standard", "full"
	// +kubebuilder:validation:Enum=minimal;standard;full
	Profile string `json:"profile"`

	// Node selector — nodes matching these labels get this profile
	NodeSelector map[string]string `json:"nodeSelector"`

	// Override specific settings (optional)
	DrainIntervalMs *int32 `json:"drainIntervalMs,omitempty"`
	GPUEnabled      *bool  `json:"gpuEnabled,omitempty"`
}

type PowerProfileStatus struct {
	// Number of nodes matching the selector
	MatchingNodes int32 `json:"matchingNodes,omitempty"`

	// List of matching node names
	NodeNames []string `json:"nodeNames,omitempty"`
}

// +kubebuilder:object:root=true
type PowerProfileList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []PowerProfile `json:"items"`
}
