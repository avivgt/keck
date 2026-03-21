# Keck Operator Makefile
# Follows Red Hat operator-sdk conventions for build, bundle, and catalog.

IMG ?= ghcr.io/avivgt/keck-operator:$(VERSION)
BUNDLE_IMG ?= ghcr.io/avivgt/keck-operator-bundle:v$(VERSION)
CATALOG_IMG ?= ghcr.io/avivgt/keck-operator-catalog:v$(VERSION)
VERSION ?= 0.1.0

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

##@ Docker

.PHONY: docker-build
docker-build: ## Build the operator container image
	docker build -t $(IMG) .

.PHONY: docker-push
docker-push: ## Push the operator container image
	docker push $(IMG)

##@ Deployment (direct — without OLM)

.PHONY: install
install: ## Install CRDs into the cluster
	kubectl apply -f config/crd/bases/

.PHONY: uninstall
uninstall: ## Remove CRDs from the cluster
	kubectl delete -f config/crd/bases/

.PHONY: deploy
deploy: install ## Deploy operator to the cluster
	kubectl apply -f config/manager/
	kubectl apply -f config/rbac/

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
	docker build -f bundle.Dockerfile -t $(BUNDLE_IMG) .

.PHONY: bundle-push
bundle-push: ## Push the OLM bundle image
	docker push $(BUNDLE_IMG)

.PHONY: bundle-validate
bundle-validate: ## Validate the OLM bundle
	operator-sdk bundle validate ./bundle

##@ OLM Catalog

.PHONY: catalog-build
catalog-build: ## Build the OLM catalog image
	opm index add \
		--bundles $(BUNDLE_IMG) \
		--tag $(CATALOG_IMG) \
		--container-tool docker

.PHONY: catalog-push
catalog-push: ## Push the OLM catalog image
	docker push $(CATALOG_IMG)

##@ OLM Install (on OpenShift or OLM-enabled cluster)

.PHONY: catalog-deploy
catalog-deploy: ## Deploy the catalog source to the cluster
	@echo "apiVersion: operators.coreos.com/v1alpha1" > /tmp/keck-catalogsource.yaml
	@echo "kind: CatalogSource" >> /tmp/keck-catalogsource.yaml
	@echo "metadata:" >> /tmp/keck-catalogsource.yaml
	@echo "  name: keck-operator-catalog" >> /tmp/keck-catalogsource.yaml
	@echo "  namespace: openshift-marketplace" >> /tmp/keck-catalogsource.yaml
	@echo "spec:" >> /tmp/keck-catalogsource.yaml
	@echo "  sourceType: grpc" >> /tmp/keck-catalogsource.yaml
	@echo "  image: $(CATALOG_IMG)" >> /tmp/keck-catalogsource.yaml
	@echo "  displayName: Keck Operator" >> /tmp/keck-catalogsource.yaml
	@echo "  publisher: Keck Project" >> /tmp/keck-catalogsource.yaml
	@echo "  updateStrategy:" >> /tmp/keck-catalogsource.yaml
	@echo "    registryPoll:" >> /tmp/keck-catalogsource.yaml
	@echo "      interval: 10m" >> /tmp/keck-catalogsource.yaml
	kubectl apply -f /tmp/keck-catalogsource.yaml

.PHONY: subscription-deploy
subscription-deploy: ## Create OLM subscription (installs the operator)
	@echo "apiVersion: operators.coreos.com/v1alpha1" > /tmp/keck-subscription.yaml
	@echo "kind: Subscription" >> /tmp/keck-subscription.yaml
	@echo "metadata:" >> /tmp/keck-subscription.yaml
	@echo "  name: keck-operator" >> /tmp/keck-subscription.yaml
	@echo "  namespace: openshift-operators" >> /tmp/keck-subscription.yaml
	@echo "spec:" >> /tmp/keck-subscription.yaml
	@echo "  channel: alpha" >> /tmp/keck-subscription.yaml
	@echo "  name: keck-operator" >> /tmp/keck-subscription.yaml
	@echo "  source: keck-operator-catalog" >> /tmp/keck-subscription.yaml
	@echo "  sourceNamespace: openshift-marketplace" >> /tmp/keck-subscription.yaml
	@echo "  installPlanApproval: Automatic" >> /tmp/keck-subscription.yaml
	kubectl apply -f /tmp/keck-subscription.yaml

##@ Scorecard

.PHONY: scorecard
scorecard: ## Run OLM scorecard tests
	operator-sdk scorecard ./bundle

##@ Full Release Pipeline

.PHONY: release
release: docker-build docker-push bundle-build bundle-push catalog-build catalog-push ## Build and push everything
	@echo "Released keck-operator $(VERSION)"
	@echo "  Operator image: $(IMG)"
	@echo "  Bundle image:   $(BUNDLE_IMG)"
	@echo "  Catalog image:  $(CATALOG_IMG)"
