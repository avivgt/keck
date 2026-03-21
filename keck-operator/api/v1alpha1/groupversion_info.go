// SPDX-License-Identifier: Apache-2.0

// +kubebuilder:object:generate=true
// +groupName=keck.io

package v1alpha1

import (
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/scheme"
)

var (
	GroupVersion = schema.GroupVersion{Group: "keck.io", Version: "v1alpha1"}

	SchemeBuilder = &scheme.Builder{GroupVersion: GroupVersion}

	AddToScheme = SchemeBuilder.AddToScheme
)

func init() {
	SchemeBuilder.Register(&KeckCluster{}, &KeckClusterList{})
	SchemeBuilder.Register(&PowerBudget{}, &PowerBudgetList{})
	SchemeBuilder.Register(&PowerProfile{}, &PowerProfileList{})
}
