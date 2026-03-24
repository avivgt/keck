// SPDX-License-Identifier: Apache-2.0

package controllers

import (
	"context"
	"testing"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client/fake"

	keckv1alpha1 "github.com/avivgt/keck/keck-operator/api/v1alpha1"
)

func newPowerBudget(name, namespace string, maxWatts float64) *keckv1alpha1.PowerBudget {
	return &keckv1alpha1.PowerBudget{
		TypeMeta: metav1.TypeMeta{
			APIVersion: "keck.io/v1alpha1",
			Kind:       "PowerBudget",
		},
		ObjectMeta: metav1.ObjectMeta{
			Name:      name,
			Namespace: namespace,
		},
		Spec: keckv1alpha1.PowerBudgetSpec{
			MaxWatts: maxWatts,
			Action:   "alert",
		},
	}
}

func TestPowerBudgetReconcileNotFound(t *testing.T) {
	scheme := newScheme()
	client := fake.NewClientBuilder().
		WithScheme(scheme).
		Build()

	reconciler := &PowerBudgetReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{
		Name:      "nonexistent",
		Namespace: "default",
	}}

	result, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("reconcile should not error for not found: %v", err)
	}
	if result.Requeue {
		t.Error("should not requeue for not found")
	}
}

func TestPowerBudgetReconcileBasic(t *testing.T) {
	scheme := newScheme()
	budget := newPowerBudget("test-budget", "production", 500.0)

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(budget).
		WithStatusSubresource(budget).
		Build()

	reconciler := &PowerBudgetReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{
		Name:      "test-budget",
		Namespace: "production",
	}}

	_, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("reconcile failed: %v", err)
	}

	// Verify status was updated
	updated := &keckv1alpha1.PowerBudget{}
	err = client.Get(context.Background(), req.NamespacedName, updated)
	if err != nil {
		t.Fatalf("failed to get budget: %v", err)
	}
	if updated.Status.LastUpdated.IsZero() {
		t.Error("LastUpdated should be set")
	}
}

func TestPowerBudgetExceeded(t *testing.T) {
	scheme := newScheme()
	budget := newPowerBudget("test-budget", "production", 100.0)
	budget.Status.CurrentWatts = 150.0 // Exceeds 100W budget

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(budget).
		WithStatusSubresource(budget).
		Build()

	reconciler := &PowerBudgetReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{
		Name:      "test-budget",
		Namespace: "production",
	}}

	_, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("reconcile failed: %v", err)
	}

	updated := &keckv1alpha1.PowerBudget{}
	_ = client.Get(context.Background(), req.NamespacedName, updated)
	if !updated.Status.Exceeded {
		t.Error("budget should be marked as exceeded")
	}
	if updated.Status.UsagePercent != "150%" {
		t.Errorf("expected 150%%, got %s", updated.Status.UsagePercent)
	}
}

func TestPowerBudgetNotExceeded(t *testing.T) {
	scheme := newScheme()
	budget := newPowerBudget("test-budget", "production", 500.0)
	budget.Status.CurrentWatts = 200.0

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(budget).
		WithStatusSubresource(budget).
		Build()

	reconciler := &PowerBudgetReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{
		Name:      "test-budget",
		Namespace: "production",
	}}

	_, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("reconcile failed: %v", err)
	}

	updated := &keckv1alpha1.PowerBudget{}
	_ = client.Get(context.Background(), req.NamespacedName, updated)
	if updated.Status.Exceeded {
		t.Error("budget should not be exceeded")
	}
	if updated.Status.UsagePercent != "40%" {
		t.Errorf("expected 40%%, got %s", updated.Status.UsagePercent)
	}
}

func TestPowerBudgetIdempotent(t *testing.T) {
	scheme := newScheme()
	budget := newPowerBudget("test-budget", "production", 500.0)

	client := fake.NewClientBuilder().
		WithScheme(scheme).
		WithObjects(budget).
		WithStatusSubresource(budget).
		Build()

	reconciler := &PowerBudgetReconciler{Client: client, Scheme: scheme}
	req := ctrl.Request{NamespacedName: types.NamespacedName{
		Name:      "test-budget",
		Namespace: "production",
	}}

	// Run twice
	_, err := reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("first reconcile failed: %v", err)
	}
	_, err = reconciler.Reconcile(context.Background(), req)
	if err != nil {
		t.Fatalf("second reconcile failed: %v", err)
	}
}
