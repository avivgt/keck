# Keck Operator Makefile
# Follows Red Hat operator-sdk conventions for build, bundle, and catalog.

IMG ?= ghcr.io/avivgt/keck-operator:$(VERSION)
BUNDLE_IMG ?= ghcr.io/avivgt/keck-operator-bundle:v$(VERSION)
CATALOG_IMG ?= ghcr.io/avivgt/keck-operator-catalog:v$(VERSION)
VERSION ?= 0.1.0

# Container tool: podman (Red Hat default), falls back to docker
CONTAINER_TOOL ?= $(shell command -v podman 2>/dev/null || echo docker)

# Tool versions
OPERATOR_SDK_VERSION ?= v1.38.0
OPM_VERSION ?= v1.48.0

.PHONY: help
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-25s\033[0m %s\n", $$1, $$2}'

##@ Build

.PHONY: build
build: ## Build the operator binary
	go build -o bin/manager cmd/main.go

.PHONY: run
run: ## Run the operator locally (outside cluster)
	go run cmd/main.go

.PHONY: test
test: ## Run tests
	go test ./... -coverprofile cover.out

.PHONY: fmt
fmt: ## Run go fmt
	go fmt ./...

.PHONY: vet
vet: ## Run go vet
	go vet ./...

##@ Container Images

.PHONY: image-build
image-build: ## Build the operator container image
	$(CONTAINER_TOOL) build -t $(IMG) .

.PHONY: image-push
image-push: ## Push the operator container image
	$(CONTAINER_TOOL) push $(IMG)

##@ Deployment (direct — without OLM)

.PHONY: install
install: ## Install CRDs into the cluster
	kubectl apply -f config/crd/bases/

.PHONY: uninstall
uninstall: ## Remove CRDs from the cluster
	kubectl delete -f config/crd/bases/

.PHONY: deploy
deploy: install ## Deploy operator to the cluster
	kubectl apply -f config/rbac/
	kubectl apply -f config/manager/

.PHONY: undeploy
undeploy: ## Remove operator from the cluster
	kubectl delete -f config/manager/
	kubectl delete -f config/rbac/
	kubectl delete -f config/crd/bases/

##@ OLM Bundle (Red Hat / OpenShift standard)

.PHONY: bundle
bundle: ## Generate the OLM bundle (CSV + CRDs + metadata)
	@echo "Bundle generated at bundle/"
	@echo "  manifests/  — CSV + CRDs"
	@echo "  metadata/   — annotations"
	@echo "  tests/      — scorecard config"

.PHONY: bundle-build
bundle-build: ## Build the OLM bundle image
	$(CONTAINER_TOOL) build -f bundle.Dockerfile -t $(BUNDLE_IMG) .

.PHONY: bundle-push
bundle-push: ## Push the OLM bundle image
	$(CONTAINER_TOOL) push $(BUNDLE_IMG)

.PHONY: bundle-validate
bundle-validate: ## Validate the OLM bundle
	operator-sdk bundle validate ./bundle

##@ OLM Catalog

.PHONY: catalog-build
catalog-build: ## Build the OLM catalog image
	opm index add \
		--bundles $(BUNDLE_IMG) \
		--tag $(CATALOG_IMG) \
		--container-tool $(CONTAINER_TOOL)

.PHONY: catalog-push
catalog-push: ## Push the OLM catalog image
	$(CONTAINER_TOOL) push $(CATALOG_IMG)

##@ OLM Install (on OpenShift or OLM-enabled cluster)

.PHONY: catalog-deploy
catalog-deploy: ## Deploy the catalog source to the cluster
	@cat <<'YAML' | sed "s|CATALOG_IMG|$(CATALOG_IMG)|" | kubectl apply -f -
	apiVersion: operators.coreos.com/v1alpha1
	kind: CatalogSource
	metadata:
	  name: keck-operator-catalog
	  namespace: openshift-marketplace
	spec:
	  sourceType: grpc
	  image: CATALOG_IMG
	  displayName: Keck Operator
	  publisher: Keck Project
	  updateStrategy:
	    registryPoll:
	      interval: 10m
	YAML

.PHONY: subscription-deploy
subscription-deploy: ## Create OLM subscription (installs the operator)
	@cat <<'YAML' | kubectl apply -f -
	apiVersion: operators.coreos.com/v1alpha1
	kind: Subscription
	metadata:
	  name: keck-operator
	  namespace: openshift-operators
	spec:
	  channel: alpha
	  name: keck-operator
	  source: keck-operator-catalog
	  sourceNamespace: openshift-marketplace
	  installPlanApproval: Automatic
	YAML

##@ Scorecard

.PHONY: scorecard
scorecard: ## Run OLM scorecard tests
	operator-sdk scorecard ./bundle

##@ Full Release Pipeline

.PHONY: release
release: image-build image-push bundle-build bundle-push catalog-build catalog-push ## Build and push everything
	@echo "Released keck-operator $(VERSION)"
	@echo "  Container tool: $(CONTAINER_TOOL)"
	@echo "  Operator image: $(IMG)"
	@echo "  Bundle image:   $(BUNDLE_IMG)"
	@echo "  Catalog image:  $(CATALOG_IMG)"
