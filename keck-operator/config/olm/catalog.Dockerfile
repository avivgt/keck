# File-Based Catalog (FBC) image for OLM.
# Contains the operator bundle reference so OLM can discover and install it.
#
# Build: oc new-build --name=keck-catalog --binary --strategy=docker -n keck-system
#        oc start-build keck-catalog --from-dir=keck-operator --follow -n keck-system

FROM registry.redhat.io/openshift4/ose-operator-registry:v4.14 AS builder

COPY config/olm/catalog.yaml /configs/keck-operator/catalog.yaml

RUN ["/bin/opm", "serve", "/configs", "--cache-dir=/tmp/cache", "--cache-only"]

FROM registry.redhat.io/openshift4/ose-operator-registry:v4.14

COPY --from=builder /configs /configs
COPY --from=builder /tmp/cache /tmp/cache

EXPOSE 50051

ENTRYPOINT ["/bin/opm"]
CMD ["serve", "/configs", "--cache-dir=/tmp/cache"]

LABEL operators.operatorframework.io.index.configs.v1=/configs
