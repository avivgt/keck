// SPDX-License-Identifier: Apache-2.0

package controllers

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"strings"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	rbacv1 "k8s.io/api/rbac/v1"
	"k8s.io/apimachinery/pkg/api/errors"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"

	keckv1alpha1 "github.com/avivgt/keck/keck-operator/api/v1alpha1"
)

const (
	keckNamespace = "keck-system"
	finalizerName = "keck.io/finalizer"
)

// KeckClusterReconciler reconciles a KeckCluster object.
// Creates/updates: Namespace, ServiceAccount, RBAC, DaemonSet (agent),
// Deployment (controller), Services.
type KeckClusterReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

// +kubebuilder:rbac:groups=keck.io,resources=keckclusters,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=keck.io,resources=keckclusters/status,verbs=get;update;patch
// +kubebuilder:rbac:groups=keck.io,resources=keckclusters/finalizers,verbs=update
// +kubebuilder:rbac:groups=apps,resources=daemonsets;deployments,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups="",resources=namespaces;serviceaccounts;services;configmaps;secrets,verbs=get;list;watch;create;update;patch;delete
// +kubebuilder:rbac:groups=rbac.authorization.k8s.io,resources=clusterroles;clusterrolebindings,verbs=get;list;watch;create;update;patch;delete

func (r *KeckClusterReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	logger := log.FromContext(ctx)

	// Fetch the KeckCluster resource
	var keck keckv1alpha1.KeckCluster
	if err := r.Get(ctx, req.NamespacedName, &keck); err != nil {
		if errors.IsNotFound(err) {
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, err
	}

	logger.Info("Reconciling KeckCluster", "name", keck.Name)

	// Handle deletion — clean up child resources
	if !keck.DeletionTimestamp.IsZero() {
		if controllerutil.ContainsFinalizer(&keck, finalizerName) {
			logger.Info("Cleaning up Keck resources")
			if err := r.cleanupResources(ctx, &keck); err != nil {
				return ctrl.Result{}, fmt.Errorf("cleanup failed: %w", err)
			}

			controllerutil.RemoveFinalizer(&keck, finalizerName)
			if err := r.Update(ctx, &keck); err != nil {
				return ctrl.Result{}, err
			}
			logger.Info("Finalizer removed, cleanup complete")
		}
		return ctrl.Result{}, nil
	}

	// Add finalizer on first reconcile
	if !controllerutil.ContainsFinalizer(&keck, finalizerName) {
		controllerutil.AddFinalizer(&keck, finalizerName)
		if err := r.Update(ctx, &keck); err != nil {
			return ctrl.Result{}, err
		}
		logger.Info("Finalizer added")
	}

	// Update status to Installing only on first deploy
	if keck.Status.Phase == "" {
		keck.Status.Phase = "Installing"
		if err := r.Status().Update(ctx, &keck); err != nil {
			logger.Error(err, "Failed to update status")
		}
	}

	// Ensure namespace
	if err := r.ensureNamespace(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring namespace: %w", err)
	}

	// Ensure ServiceAccount
	if err := r.ensureServiceAccount(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring service account: %w", err)
	}

	// Ensure API key secret for agent ↔ controller auth
	if err := r.ensureAPIKeySecret(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring API key secret: %w", err)
	}

	// Ensure RBAC
	if err := r.ensureRBAC(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring RBAC: %w", err)
	}

	// Ensure agent DaemonSet
	if err := r.ensureAgentDaemonSet(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring agent DaemonSet: %w", err)
	}

	// Ensure controller Deployment
	if err := r.ensureControllerDeployment(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring controller Deployment: %w", err)
	}

	// Ensure controller Service
	if err := r.ensureControllerService(ctx, &keck); err != nil {
		return ctrl.Result{}, fmt.Errorf("ensuring controller Service: %w", err)
	}

	// Update status
	if err := r.updateStatus(ctx, &keck); err != nil {
		logger.Error(err, "Failed to update status")
	}

	return ctrl.Result{}, nil
}

// cleanupResources deletes all child resources created by the operator.
// Called when the KeckCluster CR is being deleted (finalizer logic).
func (r *KeckClusterReconciler) cleanupResources(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	logger := log.FromContext(ctx)

	// Delete in reverse creation order: Service → Deployment → DaemonSet → Secret → RBAC → SA → Namespace

	// Controller Service
	svc := &corev1.Service{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, svc); err == nil {
		logger.Info("Deleting Service keck-controller")
		if err := r.Delete(ctx, svc); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting Service: %w", err)
		}
	}

	// Controller Deployment
	deploy := &appsv1.Deployment{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, deploy); err == nil {
		logger.Info("Deleting Deployment keck-controller")
		if err := r.Delete(ctx, deploy); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting Deployment: %w", err)
		}
	}

	// Agent DaemonSet
	ds := &appsv1.DaemonSet{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, ds); err == nil {
		logger.Info("Deleting DaemonSet keck-agent")
		if err := r.Delete(ctx, ds); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting DaemonSet: %w", err)
		}
	}

	// API key Secret
	secret := &corev1.Secret{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-api-key", Namespace: keckNamespace}, secret); err == nil {
		logger.Info("Deleting Secret keck-api-key")
		if err := r.Delete(ctx, secret); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting Secret: %w", err)
		}
	}

	// ClusterRoleBinding (cluster-scoped)
	crb := &rbacv1.ClusterRoleBinding{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-agent"}, crb); err == nil {
		logger.Info("Deleting ClusterRoleBinding keck-agent")
		if err := r.Delete(ctx, crb); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting ClusterRoleBinding: %w", err)
		}
	}

	// ClusterRole (cluster-scoped)
	cr := &rbacv1.ClusterRole{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-agent"}, cr); err == nil {
		logger.Info("Deleting ClusterRole keck-agent")
		if err := r.Delete(ctx, cr); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting ClusterRole: %w", err)
		}
	}

	// ServiceAccount
	sa := &corev1.ServiceAccount{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, sa); err == nil {
		logger.Info("Deleting ServiceAccount keck-agent")
		if err := r.Delete(ctx, sa); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting ServiceAccount: %w", err)
		}
	}

	// Namespace — delete last, cascades everything inside
	ns := &corev1.Namespace{}
	if err := r.Get(ctx, types.NamespacedName{Name: keckNamespace}, ns); err == nil {
		logger.Info("Deleting Namespace", "namespace", keckNamespace)
		if err := r.Delete(ctx, ns); err != nil && !errors.IsNotFound(err) {
			return fmt.Errorf("deleting Namespace: %w", err)
		}
	}

	return nil
}

func (r *KeckClusterReconciler) ensureNamespace(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	ns := &corev1.Namespace{
		ObjectMeta: metav1.ObjectMeta{
			Name: keckNamespace,
			Labels: map[string]string{
				"app.kubernetes.io/managed-by": "keck-operator",
				"app.kubernetes.io/part-of":    "keck",
			},
		},
	}

	existing := &corev1.Namespace{}
	err := r.Get(ctx, types.NamespacedName{Name: keckNamespace}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, ns)
	}
	return err
}

func (r *KeckClusterReconciler) ensureServiceAccount(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	sa := &corev1.ServiceAccount{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "keck-agent",
			Namespace: keckNamespace,
			Labels:    commonLabels(),
		},
	}

	existing := &corev1.ServiceAccount{}
	err := r.Get(ctx, types.NamespacedName{Name: sa.Name, Namespace: sa.Namespace}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, sa)
	}
	return err
}

func (r *KeckClusterReconciler) ensureAPIKeySecret(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	secretName := "keck-api-key"
	existing := &corev1.Secret{}
	err := r.Get(ctx, types.NamespacedName{Name: secretName, Namespace: keckNamespace}, existing)
	if err == nil {
		return nil // Secret already exists — don't regenerate
	}
	if !errors.IsNotFound(err) {
		return err
	}

	// Generate a random 32-byte API key
	keyBytes := make([]byte, 32)
	if _, err := rand.Read(keyBytes); err != nil {
		return fmt.Errorf("generating API key: %w", err)
	}
	apiKey := hex.EncodeToString(keyBytes)

	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      secretName,
			Namespace: keckNamespace,
			Labels:    commonLabels(),
		},
		Type: corev1.SecretTypeOpaque,
		StringData: map[string]string{
			"api-key": apiKey,
		},
	}
	return r.Create(ctx, secret)
}

func (r *KeckClusterReconciler) ensureRBAC(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	// ClusterRole for agent: read nodes, pods (for K8s enrichment)
	role := &rbacv1.ClusterRole{
		ObjectMeta: metav1.ObjectMeta{
			Name:   "keck-agent",
			Labels: commonLabels(),
		},
		Rules: []rbacv1.PolicyRule{
			{
				APIGroups: []string{""},
				Resources: []string{"nodes", "pods", "namespaces"},
				Verbs:     []string{"get", "list", "watch"},
			},
			{
				APIGroups: []string{"keck.io"},
				Resources: []string{"powerbudgets", "powerprofiles"},
				Verbs:     []string{"get", "list", "watch"},
			},
		},
	}

	existing := &rbacv1.ClusterRole{}
	err := r.Get(ctx, types.NamespacedName{Name: role.Name}, existing)
	if errors.IsNotFound(err) {
		if err := r.Create(ctx, role); err != nil {
			return err
		}
	}

	// ClusterRoleBinding
	binding := &rbacv1.ClusterRoleBinding{
		ObjectMeta: metav1.ObjectMeta{
			Name:   "keck-agent",
			Labels: commonLabels(),
		},
		RoleRef: rbacv1.RoleRef{
			APIGroup: "rbac.authorization.k8s.io",
			Kind:     "ClusterRole",
			Name:     "keck-agent",
		},
		Subjects: []rbacv1.Subject{
			{
				Kind:      "ServiceAccount",
				Name:      "keck-agent",
				Namespace: keckNamespace,
			},
		},
	}

	existingBinding := &rbacv1.ClusterRoleBinding{}
	err = r.Get(ctx, types.NamespacedName{Name: binding.Name}, existingBinding)
	if errors.IsNotFound(err) {
		return r.Create(ctx, binding)
	}
	return err
}

func (r *KeckClusterReconciler) ensureAgentDaemonSet(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	privileged := true
	hostPID := true

	image := fmt.Sprintf("%s:%s", imageRepo(keck), imageTag(keck))

	// Build environment variables
	envVars := []corev1.EnvVar{
		{
			Name: "NODE_NAME",
			ValueFrom: &corev1.EnvVarSource{
				FieldRef: &corev1.ObjectFieldSelector{
					FieldPath: "spec.nodeName",
				},
			},
		},
		{
			Name:  "KECK_CONTROLLER_URL",
			Value: "http://keck-controller.keck-system.svc:8080",
		},
		{
			Name: "KECK_API_KEY",
			ValueFrom: &corev1.EnvVarSource{
				SecretKeyRef: &corev1.SecretKeySelector{
					LocalObjectReference: corev1.LocalObjectReference{
						Name: "keck-api-key",
					},
					Key: "api-key",
				},
			},
		},
	}

	// Add Redfish/BMC configuration if specified
	if keck.Spec.Agent.Redfish != nil {
		rf := keck.Spec.Agent.Redfish

		// Build REDFISH_MAP from NodeBMCMap: "SERIAL1=https://ip1,SERIAL2=https://ip2"
		if len(rf.NodeBMCMap) > 0 {
			var mapEntries []string
			for _, entry := range rf.NodeBMCMap {
				mapEntries = append(mapEntries, fmt.Sprintf("%s=%s", entry.Serial, entry.Endpoint))
			}
			envVars = append(envVars, corev1.EnvVar{
				Name:  "REDFISH_MAP",
				Value: strings.Join(mapEntries, ","),
			})
		}

		// Inject credentials from Secret
		if rf.CredentialsSecret != "" {
			envVars = append(envVars,
				corev1.EnvVar{
					Name: "REDFISH_USERNAME",
					ValueFrom: &corev1.EnvVarSource{
						SecretKeyRef: &corev1.SecretKeySelector{
							LocalObjectReference: corev1.LocalObjectReference{
								Name: rf.CredentialsSecret,
							},
							Key: "username",
						},
					},
				},
				corev1.EnvVar{
					Name: "REDFISH_PASSWORD",
					ValueFrom: &corev1.EnvVarSource{
						SecretKeyRef: &corev1.SecretKeySelector{
							LocalObjectReference: corev1.LocalObjectReference{
								Name: rf.CredentialsSecret,
							},
							Key: "password",
						},
					},
				},
			)
		}
	}

	ds := &appsv1.DaemonSet{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "keck-agent",
			Namespace: keckNamespace,
			Labels:    agentLabels(),
		},
		Spec: appsv1.DaemonSetSpec{
			Selector: &metav1.LabelSelector{
				MatchLabels: map[string]string{
					"app.kubernetes.io/name":      "keck-agent",
					"app.kubernetes.io/component": "agent",
				},
			},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{
					Labels: agentLabels(),
				},
				Spec: corev1.PodSpec{
					ServiceAccountName: "keck-agent",
					HostPID:            hostPID,
					NodeSelector:       keck.Spec.Agent.NodeSelector,
					Tolerations:        agentTolerations(keck),
					Containers: []corev1.Container{
						{
							Name:            "keck-agent",
							Image:           image,
							ImagePullPolicy: keck.Spec.Image.PullPolicy,
							Command:         []string{"/usr/bin/keck-agent"},
							Env:             envVars,
							SecurityContext: &corev1.SecurityContext{
								Privileged: &privileged,
							},
							Resources: agentResources(keck),
							VolumeMounts: []corev1.VolumeMount{
								{Name: "proc", MountPath: "/host/proc", ReadOnly: true},
								{Name: "sys", MountPath: "/host/sys", ReadOnly: true},
							},
							Ports: []corev1.ContainerPort{
								{Name: "metrics", ContainerPort: 9100, Protocol: corev1.ProtocolTCP},
							},
						},
					},
					Volumes: []corev1.Volume{
						{
							Name:         "proc",
							VolumeSource: corev1.VolumeSource{HostPath: &corev1.HostPathVolumeSource{Path: "/proc"}},
						},
						{
							Name:         "sys",
							VolumeSource: corev1.VolumeSource{HostPath: &corev1.HostPathVolumeSource{Path: "/sys"}},
						},
					},
				},
			},
		},
	}

	existing := &appsv1.DaemonSet{}
	err := r.Get(ctx, types.NamespacedName{Name: ds.Name, Namespace: ds.Namespace}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, ds)
	}
	if err != nil {
		return err
	}

	// Update existing
	existing.Spec = ds.Spec
	return r.Update(ctx, existing)
}

func (r *KeckClusterReconciler) ensureControllerDeployment(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	image := fmt.Sprintf("%s:%s", imageRepo(keck), imageTag(keck))
	replicas := keck.Spec.Controller.Replicas
	if replicas == 0 {
		replicas = 1
	}

	args := []string{}
	if keck.Spec.Controller.SchedulerEnabled {
		args = append(args, "--scheduler-enabled")
	}
	if keck.Spec.Controller.CarbonAPIEndpoint != "" {
		args = append(args, fmt.Sprintf("--carbon-api=%s", keck.Spec.Controller.CarbonAPIEndpoint))
	}
	if keck.Spec.Controller.CarbonRegion != "" {
		args = append(args, fmt.Sprintf("--carbon-region=%s", keck.Spec.Controller.CarbonRegion))
	}
	if keck.Spec.FleetEndpoint != "" {
		args = append(args, fmt.Sprintf("--fleet-endpoint=%s", keck.Spec.FleetEndpoint))
	}

	deploy := &appsv1.Deployment{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "keck-controller",
			Namespace: keckNamespace,
			Labels:    controllerLabels(),
		},
		Spec: appsv1.DeploymentSpec{
			Replicas: &replicas,
			Selector: &metav1.LabelSelector{
				MatchLabels: map[string]string{
					"app.kubernetes.io/name":      "keck-controller",
					"app.kubernetes.io/component": "controller",
				},
			},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{
					Labels: controllerLabels(),
				},
				Spec: corev1.PodSpec{
					ServiceAccountName: "keck-agent",
					Containers: []corev1.Container{
						{
							Name:            "keck-controller",
							Image:           image,
							ImagePullPolicy: keck.Spec.Image.PullPolicy,
							Command:         []string{"/usr/bin/keck-controller"},
							Args:            args,
							Ports: []corev1.ContainerPort{
								{Name: "grpc", ContainerPort: 9090, Protocol: corev1.ProtocolTCP},
								{Name: "http", ContainerPort: 8080, Protocol: corev1.ProtocolTCP},
							},
							Env: []corev1.EnvVar{
								{
									Name: "KECK_API_KEY",
									ValueFrom: &corev1.EnvVarSource{
										SecretKeyRef: &corev1.SecretKeySelector{
											LocalObjectReference: corev1.LocalObjectReference{
												Name: "keck-api-key",
											},
											Key: "api-key",
										},
									},
								},
							},
							Resources: controllerResources(keck),
						},
					},
				},
			},
		},
	}

	existing := &appsv1.Deployment{}
	err := r.Get(ctx, types.NamespacedName{Name: deploy.Name, Namespace: deploy.Namespace}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, deploy)
	}
	if err != nil {
		return err
	}

	existing.Spec = deploy.Spec
	return r.Update(ctx, existing)
}

func (r *KeckClusterReconciler) ensureControllerService(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	svc := &corev1.Service{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "keck-controller",
			Namespace: keckNamespace,
			Labels:    controllerLabels(),
		},
		Spec: corev1.ServiceSpec{
			Selector: map[string]string{
				"app.kubernetes.io/name":      "keck-controller",
				"app.kubernetes.io/component": "controller",
			},
			Ports: []corev1.ServicePort{
				{Name: "grpc", Port: 9090, Protocol: corev1.ProtocolTCP},
				{Name: "http", Port: 8080, Protocol: corev1.ProtocolTCP},
			},
		},
	}

	existing := &corev1.Service{}
	err := r.Get(ctx, types.NamespacedName{Name: svc.Name, Namespace: svc.Namespace}, existing)
	if errors.IsNotFound(err) {
		return r.Create(ctx, svc)
	}
	return err
}

func (r *KeckClusterReconciler) updateStatus(ctx context.Context, keck *keckv1alpha1.KeckCluster) error {
	// Check DaemonSet status
	ds := &appsv1.DaemonSet{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-agent", Namespace: keckNamespace}, ds); err == nil {
		keck.Status.AgentReady = ds.Status.NumberReady
		keck.Status.AgentDesired = ds.Status.DesiredNumberScheduled
	}

	// Check Deployment status
	deploy := &appsv1.Deployment{}
	if err := r.Get(ctx, types.NamespacedName{Name: "keck-controller", Namespace: keckNamespace}, deploy); err == nil {
		keck.Status.ControllerReady = deploy.Status.ReadyReplicas > 0
	}

	// Determine phase
	if keck.Status.AgentReady > 0 && keck.Status.ControllerReady {
		keck.Status.Phase = "Running"
		keck.Status.Message = fmt.Sprintf(
			"%d/%d agents ready, controller running",
			keck.Status.AgentReady, keck.Status.AgentDesired,
		)
	} else {
		keck.Status.Phase = "Installing"
		keck.Status.Message = "Waiting for components to become ready"
	}

	keck.Status.LastUpdated = metav1.Now()
	return r.Status().Update(ctx, keck)
}

func (r *KeckClusterReconciler) SetupWithManager(mgr ctrl.Manager) error {
	return ctrl.NewControllerManagedBy(mgr).
		For(&keckv1alpha1.KeckCluster{}).
		Owns(&appsv1.DaemonSet{}).
		Owns(&appsv1.Deployment{}).
		Complete(r)
}

// ─── Helpers ─────────────────────────────────────────────────────

func commonLabels() map[string]string {
	return map[string]string{
		"app.kubernetes.io/managed-by": "keck-operator",
		"app.kubernetes.io/part-of":    "keck",
	}
}

func agentLabels() map[string]string {
	labels := commonLabels()
	labels["app.kubernetes.io/name"] = "keck-agent"
	labels["app.kubernetes.io/component"] = "agent"
	return labels
}

func controllerLabels() map[string]string {
	labels := commonLabels()
	labels["app.kubernetes.io/name"] = "keck-controller"
	labels["app.kubernetes.io/component"] = "controller"
	return labels
}

func imageRepo(keck *keckv1alpha1.KeckCluster) string {
	if keck.Spec.Image.Repository != "" {
		return keck.Spec.Image.Repository
	}
	return "quay.io/aguetta/keck"
}

func imageTag(keck *keckv1alpha1.KeckCluster) string {
	if keck.Spec.Image.Tag != "" {
		return keck.Spec.Image.Tag
	}
	return "latest"
}

func agentTolerations(keck *keckv1alpha1.KeckCluster) []corev1.Toleration {
	if len(keck.Spec.Agent.Tolerations) > 0 {
		return keck.Spec.Agent.Tolerations
	}
	// Default: tolerate control plane nodes so we meter everything
	return []corev1.Toleration{
		{
			Key:      "node-role.kubernetes.io/control-plane",
			Operator: corev1.TolerationOpExists,
			Effect:   corev1.TaintEffectNoSchedule,
		},
		{
			Key:      "node-role.kubernetes.io/master",
			Operator: corev1.TolerationOpExists,
			Effect:   corev1.TaintEffectNoSchedule,
		},
	}
}

func agentResources(keck *keckv1alpha1.KeckCluster) corev1.ResourceRequirements {
	if keck.Spec.Agent.Resources.Limits != nil {
		return keck.Spec.Agent.Resources
	}
	// Defaults based on Standard profile
	return corev1.ResourceRequirements{
		Requests: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("50m"),
			corev1.ResourceMemory: resource.MustParse("64Mi"),
		},
		Limits: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("200m"),
			corev1.ResourceMemory: resource.MustParse("256Mi"),
		},
	}
}

func controllerResources(keck *keckv1alpha1.KeckCluster) corev1.ResourceRequirements {
	if keck.Spec.Controller.Resources.Limits != nil {
		return keck.Spec.Controller.Resources
	}
	return corev1.ResourceRequirements{
		Requests: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("100m"),
			corev1.ResourceMemory: resource.MustParse("128Mi"),
		},
		Limits: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("500m"),
			corev1.ResourceMemory: resource.MustParse("512Mi"),
		},
	}
}

// Ensure unused imports are used
var _ = controllerutil.CreateOrUpdate
