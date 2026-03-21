// SPDX-License-Identifier: Apache-2.0

package controllers

import (
	"context"
	"fmt"

	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/log"

	keckv1alpha1 "github.com/avivgt/keck/keck-operator/api/v1alpha1"
)

// PowerBudgetReconciler reconciles PowerBudget objects.
// Syncs budget configuration to the keck-controller, which enforces
// them via the scheduler extender.
type PowerBudgetReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// +kubebuilder:rbac:groups=keck.io,resources=powerbudgets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=keck.io,resources=powerbudgets/status,verbs=get;update;patch

func (r *PowerBudgetReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	var budget keckv1alpha1.PowerBudget
	if err := r.Get(ctx, req.NamespacedName, &budget); err != nil {
		return ctrl.Result{}, client.IgnoreNotFound(err)
	}

	logger.Info("Reconciling PowerBudget",
		"namespace", budget.Namespace,
		"maxWatts", budget.Spec.MaxWatts,
		"action", budget.Spec.Action,
	)

	// TODO: Query keck-controller API for current namespace power usage
	// and update the PowerBudget status.
	//
	// GET http://keck-controller.keck-system:8080/api/v1/namespaces/{ns}
	// → { "total_watts": 1234.5, ... }
	//
	// For now, set placeholder status
	budget.Status.LastUpdated = metav1.Now()

	if budget.Status.CurrentWatts > budget.Spec.MaxWatts {
		budget.Status.Exceeded = true
		budget.Status.UsagePercent = fmt.Sprintf("%.0f%%",
			(budget.Status.CurrentWatts/budget.Spec.MaxWatts)*100)

		logger.Info("PowerBudget exceeded",
			"namespace", budget.Namespace,
			"current", budget.Status.CurrentWatts,
			"max", budget.Spec.MaxWatts,
		)

		// TODO: Execute action (alert webhook, etc.)
	} else {
		budget.Status.Exceeded = false
		if budget.Spec.MaxWatts > 0 {
			budget.Status.UsagePercent = fmt.Sprintf("%.0f%%",
				(budget.Status.CurrentWatts/budget.Spec.MaxWatts)*100)
		}
	}

	if err := r.Status().Update(ctx, &budget); err != nil {
		logger.Error(err, "Failed to update PowerBudget status")
		return ctrl.Result{}, err
	}

	return ctrl.Result{}, nil
}

func (r *PowerBudgetReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&keckv1alpha1.PowerBudget{}).
		Complete(r)
}
