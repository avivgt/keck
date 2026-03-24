// SPDX-License-Identifier: Apache-2.0

package controllers

import (
	"context"
	"testing"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	keckv1alpha1 "github.com/avivgt/keck/keck-operator/api/v1alpha1"
)

func newScheme() *runtime.Scheme {
	s := runtime.NewScheme()
	_ = clientgoscheme.AddToScheme(s)
	_ = keckv1alpha1.AddToScheme(s)
	_ = appsv1.AddToScheme(s)
	_ = rbacv1.AddToScheme(s)
	return s
}

func newKeckCluster(name string) *keckv1alpha1.KeckCluster {
	return &keckv1alpha1.KeckCluster{
		TypeMeta: metav1.TypeMeta{
			APIVersion: "keck.io/v1alpha1",
			Kind:       "KeckCluster",
		},
		ObjectMeta: metav1.ObjectMeta{
			Name: name,
		},
		Spec: keckv1alpha1.KeckClusterSpec{
			Agent: keckv1alpha1.AgentSpec{
				DefaultProfile: "standard",
			},
			Controller: keckv1alpha1.ControllerSpec{
				Replicas: 1,
			},
			Image: keckv1alpha1.ImageSpec{
				Repository: "ghcr.io/avivgt/keck",
				Tag:        "v0.1.0",
				PullPolicy: corev1.PullIfNotPresent,
			},
		},
	}
}

func TestHelperFunctions(t *testing.T) {
	t.Run("commonLabels", func(t *testing.T) {
		labels := commonLabels()
		if labels["app.kubernetes.io/managed-by"] != "keck-operator" {
			t.Error("expected managed-by label")
		}
		if labels["app.kubernetes.io/part-of"] != "keck" {
			t.Error("expected part-of label")
		}
	})

	t.Run("agentLabels", func(t *testing.T) {
		labels := agentLabels()
		if labels["app.kubernetes.io/name"] != "keck-agent" {
			t.Error("expected agent name label")
		}
		if labels["app.kubernetes.io/component"] != "agent" {
			t.Error("expected agent component label")
		}
		// Should also have common labels
		if labels["app.kubernetes.io/managed-by"] != "keck-operator" {
			t.Error("expected managed-by from common labels")
		}
	})

	t.Run("controllerLabels", func(t *testing.T) {
		labels := controllerLabels()
		if labels["app.kubernetes.io/name"] != "keck-controller" {
			t.Error("expected controller name label")
		}
		if labels["app.kubernetes.io/component"] != "controller" {
			t.Error("expected controller component label")
		}
	})

	t.Run("imageRepo_default", func(t *testing.T) {
		keck := newKeckCluster("test")
		keck.Spec.Image.Repository = ""
		repo := imageRepo(keck)
		if repo != "ghcr.io/avivgt/keck" {
			t.Errorf("expected default repo, got %s", repo)
		}
	})

	t.Run("imageRepo_custom", func(t *testing.T) {
		keck := newKeckCluster("test")
		keck.Spec.Image.Repository = "custom.io/keck"
		repo := imageRepo(keck)
		if repo != "custom.io/keck" {
			t.Errorf("expected custom repo, got %s", repo)
		}
	})

	t.Run("imageTag_default", func(t *testing.T) {
		keck := newKeckCluster("test")
		keck.Spec.Image.Tag = ""
		tag := imageTag(keck)
		if tag != "latest" {
			t.Errorf("expected latest tag, got %s", tag)
		}
	})

	t.Run("imageTag_custom", func(t *testing.T) {
		keck := newKeckCluster("test")
		keck.Spec.Image.Tag = "v1.2.3"
		tag := imageTag(keck)
		if tag != "v1.2.3" {
			t.Errorf("expected v1.2.3, got %s", tag)
		}
	})

	t.Run("agentTolerations_default", func(t *testing.T) {
		keck := newKeckCluster("test")
		tols := agentTolerations(keck)
		if len(tols) != 2 {
			t.Fatalf("expected 2 default tolerations, got %d", len(tols))
		}
		if tols[0].Key != "node-role.kubernetes.io/control-plane" {
			t.Error("expected control-plane toleration")
		}
		if tols[1].Key != "node-role.kubernetes.io/master" {
			t.Error("expected master toleration")
		}
	})

	t.Run("agentTolerations_custom", func(t *testing.T) {
		keck := newKeckCluster("test")
		keck.Spec.Agent.Tolerations = []corev1.Toleration{
			{Key: "custom-key", Operator: corev1.TolerationOpExists},
		}
		tols := agentTolerations(keck)
		if len(tols) != 1 {
			t.Fatalf("expected 1 custom toleration, got %d", len(tols))
		}
		if tols[0].Key != "custom-key" {
			t.Error("expected custom toleration key")
		}
	})

	t.Run("agentResources_default", func(t *testing.T) {
		keck := newKeckCluster("test")
		res := agentResources(keck)
		if res.Limits == nil {
			t.Error("expected default resource limits")
		}
		cpuLimit := res.Limits[corev1.ResourceCPU]
		if cpuLimit.String() != "200m" {
			t.Errorf("expected 200m CPU limit, got %s", cpuLimit.String())
		}
	})

	t.Run("controllerResources_default", func(t *testing.T) {
		keck := newKeckCluster("test")
		res := controllerResources(keck)
		if res.Limits == nil {
			t.Error("expected default controller resource limits")
		}
		memLimit := res.Limits[corev1.ResourceMemory]
		if memLimit.String() != "512Mi" {
			t.Errorf("expected 512Mi memory limit, got %s", memLimit.String())
		}
	})
}

func TestReconcileCreatesNamespace(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{
		Client: client,
		Scheme: scheme,
	}

	ctx := context.Background()
	err := reconciler.ensureNamespace(ctx, keck)
	if err != nil {
		t.Fatalf("ensureNamespace failed: %v", err)
	}

	// Verify namespace was created
	ns := &corev1.Namespace{}
	err = client.Get(ctx, types.NamespacedName{Name: keckNamespace}, ns)
	if err != nil {
		t.Fatalf("namespace not created: %v", err)
	}
	if ns.Labels["app.kubernetes.io/managed-by"] != "keck-operator" {
		t.Error("namespace missing managed-by label")
	}
}

func TestReconcileCreatesServiceAccount(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")

	// Pre-create namespace
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}
	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureServiceAccount(ctx, keck)
	if err != nil {
		t.Fatalf("ensureServiceAccount failed: %v", err)
	}

	sa := &corev1.ServiceAccount{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, sa)
	if err != nil {
		t.Fatalf("service account not created: %v", err)
	}
}

func TestReconcileCreatesRBAC(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureRBAC(ctx, keck)
	if err != nil {
		t.Fatalf("ensureRBAC failed: %v", err)
	}

	// Check ClusterRole
	role := &rbacv1.ClusterRole{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-agent"}, role)
	if err != nil {
		t.Fatalf("ClusterRole not created: %v", err)
	}
	if len(role.Rules) != 2 {
		t.Errorf("expected 2 RBAC rules, got %d", len(role.Rules))
	}

	// Check ClusterRoleBinding
	binding := &rbacv1.ClusterRoleBinding{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-agent"}, binding)
	if err != nil {
		t.Fatalf("ClusterRoleBinding not created: %v", err)
	}
	if binding.RoleRef.Name != "keck-agent" {
		t.Error("binding doesn't reference correct role")
	}
}

func TestReconcileIdempotent(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}

	// Run reconcile twice — should not error
	req := ctrl.Request{NamespacedName: types.NamespacedName{Name: "test-cluster"}}
	ctx := context.Background()

	_, err := reconciler.Reconcile(ctx, req)
	if err != nil {
		t.Fatalf("first reconcile failed: %v", err)
	}

	_, err = reconciler.Reconcile(ctx, req)
	if err != nil {
		t.Fatalf("second reconcile (idempotent) failed: %v", err)
	}
}

func TestReconcileNotFound(t *testing.T) {
	scheme := newScheme()
	client := fake.NewClientBuilder().
		WithScheme(scheme).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{Name: "nonexistent"}}

	result, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("reconcile should not error for not found: %v", err)
	}
	if result.Requeue {
		t.Error("should not requeue for not found")
	}
}

func TestReconcileCreatesAgentDaemonSet(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureAgentDaemonSet(ctx, keck)
	if err != nil {
		t.Fatalf("ensureAgentDaemonSet failed: %v", err)
	}

	ds := &appsv1.DaemonSet{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, ds)
	if err != nil {
		t.Fatalf("DaemonSet not created: %v", err)
	}

	// Verify key properties
	if ds.Spec.Template.Spec.HostPID != true {
		t.Error("agent DaemonSet should have hostPID=true")
	}
	if len(ds.Spec.Template.Spec.Containers) != 1 {
		t.Fatalf("expected 1 container, got %d", len(ds.Spec.Template.Spec.Containers))
	}
	container := ds.Spec.Template.Spec.Containers[0]
	if container.Name != "keck-agent" {
		t.Errorf("expected container name keck-agent, got %s", container.Name)
	}
	if *container.SecurityContext.Privileged != true {
		t.Error("agent container should be privileged")
	}

	// Check volume mounts for /proc and /sys
	if len(container.VolumeMounts) != 2 {
		t.Errorf("expected 2 volume mounts, got %d", len(container.VolumeMounts))
	}
}

func TestReconcileCreatesControllerDeployment(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureControllerDeployment(ctx, keck)
	if err != nil {
		t.Fatalf("ensureControllerDeployment failed: %v", err)
	}

	deploy := &appsv1.Deployment{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, deploy)
	if err != nil {
		t.Fatalf("Deployment not created: %v", err)
	}

	if *deploy.Spec.Replicas != 1 {
		t.Errorf("expected 1 replica, got %d", *deploy.Spec.Replicas)
	}
}

func TestReconcileCreatesControllerService(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureControllerService(ctx, keck)
	if err != nil {
		t.Fatalf("ensureControllerService failed: %v", err)
	}

	svc := &corev1.Service{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, svc)
	if err != nil {
		t.Fatalf("Service not created: %v", err)
	}

	if len(svc.Spec.Ports) != 2 {
		t.Errorf("expected 2 service ports (grpc, http), got %d", len(svc.Spec.Ports))
	}
}

func TestDaemonSetWithRedfish(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	keck.Spec.Agent.Redfish = &keckv1alpha1.RedfishSpec{
		CredentialsSecret: "bmc-creds",
		NodeBMCMap: []keckv1alpha1.NodeBMCEntry{
			{Serial: "SN001", Endpoint: "https://192.168.1.10"},
			{Serial: "SN002", Endpoint: "https://192.168.1.11"},
		},
	}
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureAgentDaemonSet(ctx, keck)
	if err != nil {
		t.Fatalf("ensureAgentDaemonSet with Redfish failed: %v", err)
	}

	ds := &appsv1.DaemonSet{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, ds)
	if err != nil {
		t.Fatalf("DaemonSet not found: %v", err)
	}

	container := ds.Spec.Template.Spec.Containers[0]

	// Check REDFISH_MAP env var exists
	hasRedfishMap := false
	hasRedfishUser := false
	hasRedfishPass := false
	for _, env := range container.Env {
		switch env.Name {
		case "REDFISH_MAP":
			hasRedfishMap = true
		case "REDFISH_USERNAME":
			hasRedfishUser = true
			if env.ValueFrom == nil || env.ValueFrom.SecretKeyRef == nil {
				t.Error("REDFISH_USERNAME should come from secret")
			} else if env.ValueFrom.SecretKeyRef.Name != "bmc-creds" {
				t.Error("REDFISH_USERNAME should reference bmc-creds secret")
			}
		case "REDFISH_PASSWORD":
			hasRedfishPass = true
		}
	}

	if !hasRedfishMap {
		t.Error("missing REDFISH_MAP env var")
	}
	if !hasRedfishUser {
		t.Error("missing REDFISH_USERNAME env var")
	}
	if !hasRedfishPass {
		t.Error("missing REDFISH_PASSWORD env var")
	}
}

func TestControllerDeploymentDefaultReplicas(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	keck.Spec.Controller.Replicas = 0 // should default to 1
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureControllerDeployment(ctx, keck)
	if err != nil {
		t.Fatalf("failed: %v", err)
	}

	deploy := &appsv1.Deployment{}
	err = client.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, deploy)
	if err != nil {
		t.Fatalf("Deployment not found: %v", err)
	}
	if *deploy.Spec.Replicas != 1 {
		t.Errorf("expected default 1 replica, got %d", *deploy.Spec.Replicas)
	}
}

func TestControllerDeploymentWithArgs(t *testing.T) {
	scheme := newScheme()
	keck := newKeckCluster("test-cluster")
	keck.Spec.Controller.SchedulerEnabled = true
	keck.Spec.Controller.CarbonAPIEndpoint = "https://api.watttime.org"
	keck.Spec.Controller.CarbonRegion = "US-CAL-CISO"
	keck.Spec.FleetEndpoint = "https://fleet.example.com"
	ns := &corev1.Namespace{ObjectMeta: metav1.ObjectMeta{Name: keckNamespace}}

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(keck, ns).
		WithStatusSubresource(keck).
		Build()

	reconciler := &KeckClusterReconciler{Client: client, Scheme: scheme}
	ctx := context.Background()

	err := reconciler.ensureControllerDeployment(ctx, keck)
	if err != nil {
		t.Fatalf("failed: %v", err)
	}

	deploy := &appsv1.Deployment{}
	_ = client.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, deploy)

	args := deploy.Spec.Template.Spec.Containers[0].Args
	hasScheduler := false
	hasCarbonAPI := false
	hasCarbonRegion := false
	hasFleet := false
	for _, arg := range args {
		switch {
		case arg == "--scheduler-enabled":
			hasScheduler = true
		case arg == "--carbon-api=https://api.watttime.org":
			hasCarbonAPI = true
		case arg == "--carbon-region=US-CAL-CISO":
			hasCarbonRegion = true
		case arg == "--fleet-endpoint=https://fleet.example.com":
			hasFleet = true
		}
	}

	if !hasScheduler {
		t.Error("missing --scheduler-enabled arg")
	}
	if !hasCarbonAPI {
		t.Error("missing --carbon-api arg")
	}
	if !hasCarbonRegion {
		t.Error("missing --carbon-region arg")
	}
	if !hasFleet {
		t.Error("missing --fleet-endpoint arg")
	}
}
