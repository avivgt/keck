// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct PodIdentity {
    pub name: String,
    pub namespace: String,
    pub workload_uid: String,
    pub workload_name: String,
    pub workload_kind: String,
    pub workload_category: WorkloadCategory,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkloadCategory {
    Platform,
    Operator,
    Application,
}

impl WorkloadCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Operator => "operator",
            Self::Application => "application",
        }
    }
}

pub const DEFAULT_CAPTURED_LABELS: &[&str] = &[
    "app.kubernetes.io/name",
    "app.kubernetes.io/part-of",
    "app.kubernetes.io/component",
    "argocd.argoproj.io/instance",
    "olm.owner",
];

pub const DEFAULT_CAPTURED_PREFIXES: &[&str] = &[
    "operators.coreos.com/",
];

pub fn parse_label_config(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut exact = Vec::new();
    let mut prefixes = Vec::new();
    for entry in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some(prefix) = entry.strip_suffix("/*") {
            prefixes.push(format!("{}/", prefix));
        } else {
            exact.push(entry.to_string());
        }
    }
    (exact, prefixes)
}

pub fn capture_labels(
    pod_labels: &std::collections::BTreeMap<String, String>,
    exact_keys: &[String],
    prefix_keys: &[String],
) -> HashMap<String, String> {
    let mut captured = HashMap::new();
    for key in exact_keys {
        if let Some(val) = pod_labels.get(key.as_str()) {
            captured.insert(key.clone(), val.clone());
        }
    }
    for prefix in prefix_keys {
        for (key, val) in pod_labels {
            if key.starts_with(prefix.as_str()) {
                captured.insert(key.clone(), val.clone());
            }
        }
    }
    captured
}

/// Operator namespace patterns: namespaces where OLM installs operator controllers.
const OPERATOR_NAMESPACES: &[&str] = &[
    "openshift-operators",
    "openshift-operator-lifecycle-manager",
    "openshift-marketplace",
];

/// Platform namespace prefixes: core OpenShift infrastructure.
const PLATFORM_PREFIXES: &[&str] = &[
    "openshift-",
    "kube-",
];

pub fn classify_category(
    namespace: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> WorkloadCategory {
    if OPERATOR_NAMESPACES.contains(&namespace) {
        return WorkloadCategory::Operator;
    }
    if labels.keys().any(|k| k.starts_with("operators.coreos.com/")) {
        return WorkloadCategory::Operator;
    }
    if labels.get("olm.owner").is_some() {
        return WorkloadCategory::Operator;
    }
    if PLATFORM_PREFIXES.iter().any(|p| namespace.starts_with(p)) {
        return WorkloadCategory::Platform;
    }
    if namespace == "kube-system" {
        return WorkloadCategory::Platform;
    }
    WorkloadCategory::Application
}

#[derive(Clone, Debug)]
pub struct OwnerMapping {
    pub uid: String,
    pub name: String,
    pub kind: String,
}

pub fn select_controller_owner(
    owner_refs: &[k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference],
) -> Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference> {
    owner_refs
        .iter()
        .find(|o| o.controller == Some(true))
        .or_else(|| owner_refs.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse_label_config_exact_and_prefix() {
        let (exact, prefixes) = parse_label_config(
            "app.kubernetes.io/name, operators.coreos.com/*, custom/team"
        );
        assert_eq!(exact, vec!["app.kubernetes.io/name", "custom/team"]);
        assert_eq!(prefixes, vec!["operators.coreos.com/"]);
    }

    #[test]
    fn test_parse_label_config_empty() {
        let (exact, prefixes) = parse_label_config("");
        assert!(exact.is_empty());
        assert!(prefixes.is_empty());
    }

    #[test]
    fn test_capture_labels_exact() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("app.kubernetes.io/name".into(), "my-api".into());
        pod_labels.insert("unrelated".into(), "ignored".into());
        let exact = vec!["app.kubernetes.io/name".to_string()];
        let captured = capture_labels(&pod_labels, &exact, &[]);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured["app.kubernetes.io/name"], "my-api");
    }

    #[test]
    fn test_capture_labels_prefix() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("operators.coreos.com/elastic".into(), "".into());
        pod_labels.insert("operators.coreos.com/prometheus".into(), "".into());
        pod_labels.insert("other/label".into(), "val".into());
        let prefixes = vec!["operators.coreos.com/".to_string()];
        let captured = capture_labels(&pod_labels, &[], &prefixes);
        assert_eq!(captured.len(), 2);
        assert!(captured.contains_key("operators.coreos.com/elastic"));
        assert!(captured.contains_key("operators.coreos.com/prometheus"));
    }

    #[test]
    fn test_capture_labels_no_matches() {
        let pod_labels = BTreeMap::new();
        let exact = vec!["app.kubernetes.io/name".to_string()];
        let captured = capture_labels(&pod_labels, &exact, &[]);
        assert!(captured.is_empty());
    }

    #[test]
    fn test_classify_platform_openshift() {
        let labels = BTreeMap::new();
        assert_eq!(classify_category("openshift-monitoring", &labels), WorkloadCategory::Platform);
    }

    #[test]
    fn test_classify_platform_kube_system() {
        let labels = BTreeMap::new();
        assert_eq!(classify_category("kube-system", &labels), WorkloadCategory::Platform);
    }

    #[test]
    fn test_classify_operator_by_label() {
        let mut labels = BTreeMap::new();
        labels.insert("operators.coreos.com/elasticsearch-operator".into(), "".into());
        assert_eq!(classify_category("default", &labels), WorkloadCategory::Operator);
    }

    #[test]
    fn test_classify_operator_by_olm_owner() {
        let mut labels = BTreeMap::new();
        labels.insert("olm.owner".into(), "my-operator.v1.0".into());
        assert_eq!(classify_category("my-operator-ns", &labels), WorkloadCategory::Operator);
    }

    #[test]
    fn test_classify_operator_namespace() {
        let labels = BTreeMap::new();
        assert_eq!(classify_category("openshift-operators", &labels), WorkloadCategory::Operator);
        assert_eq!(classify_category("openshift-operator-lifecycle-manager", &labels), WorkloadCategory::Operator);
        assert_eq!(classify_category("openshift-marketplace", &labels), WorkloadCategory::Operator);
    }

    #[test]
    fn test_classify_application() {
        let labels = BTreeMap::new();
        assert_eq!(classify_category("my-app-ns", &labels), WorkloadCategory::Application);
    }

    #[test]
    fn test_workload_category_as_str() {
        assert_eq!(WorkloadCategory::Platform.as_str(), "platform");
        assert_eq!(WorkloadCategory::Operator.as_str(), "operator");
        assert_eq!(WorkloadCategory::Application.as_str(), "application");
    }
}
