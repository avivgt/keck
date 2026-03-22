// SPDX-License-Identifier: Apache-2.0

// Pod-level power for a specific namespace. Drill-down from ClusterOverview.

import * as React from "react";
import { Link } from "react-router-dom";
import {
  Page,
  PageSection,
  Title,
  Breadcrumb,
  BreadcrumbItem,
  Spinner,
  EmptyState,
  EmptyStateBody,
} from "@patternfly/react-core";
import {
  Table,
  Thead,
  Tbody,
  Tr,
  Th,
  Td,
} from "@patternfly/react-table";
import { api, PodPower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

/** Extract namespace from URL path: /power-management/namespaces/<ns> */
function getNamespaceFromURL(): string {
  const path = window.location.pathname;
  const prefix = "/power-management/namespaces/";
  const idx = path.indexOf(prefix);
  if (idx >= 0) {
    const rest = path.slice(idx + prefix.length);
    // Take everything up to the next / or end
    const ns = rest.split("/")[0];
    return decodeURIComponent(ns);
  }
  return "";
}

const NamespaceView: React.FC = () => {
  const [ns, setNs] = React.useState(() => getNamespaceFromURL());
  const [pods, setPods] = React.useState<PodPower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<string | null>(null);

  // Update ns if URL changes
  React.useEffect(() => {
    const checkUrl = () => {
      const current = getNamespaceFromURL();
      if (current && current !== ns) {
        setNs(current);
      }
    };
    checkUrl();
    // Poll for URL changes (console plugin routing may not trigger re-renders)
    const interval = setInterval(checkUrl, 500);
    return () => clearInterval(interval);
  }, [ns]);

  React.useEffect(() => {
    if (!ns) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);

    const fetchData = () => {
      api.getNamespacePods(ns)
        .then((data) => {
          setPods(data);
          setError(null);
        })
        .catch((e) => {
          setError(e.message);
        })
        .finally(() => setLoading(false));
    };

    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [ns]);

  if (!ns) {
    return (
      <Page>
        <PageSection>
          <EmptyState>
            <EmptyStateBody>No namespace specified in URL.</EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  if (loading) {
    return <Page><PageSection><Spinner aria-label="Loading pods" /></PageSection></Page>;
  }

  if (error) {
    return (
      <Page>
        <PageSection>
          <EmptyState>
            <Title headingLevel="h2" size="lg">Error loading pods</Title>
            <EmptyStateBody>{error}</EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  const totalWatts = pods.reduce((sum, p) => sum + p.total_watts, 0);

  return (
    <Page>
      <PageSection>
        <Breadcrumb>
          <BreadcrumbItem>
            <Link to="/power-management">Power Management</Link>
          </BreadcrumbItem>
          <BreadcrumbItem>
            <Link to="/power-management/namespaces">Namespaces</Link>
          </BreadcrumbItem>
          <BreadcrumbItem isActive>{ns}</BreadcrumbItem>
        </Breadcrumb>

        <Title headingLevel="h1" size="xl" style={{ marginTop: 16 }}>
          {ns}
        </Title>
        <p style={{ color: "var(--pf-v6-global--Color--200)" }}>
          {pods.length} pods, {formatWatts(totalWatts)} total
        </p>
      </PageSection>

      <PageSection>
        {pods.length > 0 ? (
          <Table aria-label="Pod power table" variant="compact">
            <Thead>
              <Tr>
                <Th>Pod</Th>
                <Th>Node</Th>
                <Th>Total</Th>
                <Th>CPU</Th>
                <Th>Memory</Th>
                <Th>GPU</Th>
              </Tr>
            </Thead>
            <Tbody>
              {pods.map((pod) => (
                <Tr key={pod.pod_uid}>
                  <Td>{pod.pod_name}</Td>
                  <Td>{pod.node_name}</Td>
                  <Td>{formatWatts(pod.total_watts)}</Td>
                  <Td>{formatWatts(pod.cpu_watts)}</Td>
                  <Td>{formatWatts(pod.memory_watts)}</Td>
                  <Td>{formatWatts(pod.gpu_watts)}</Td>
                </Tr>
              ))}
            </Tbody>
          </Table>
        ) : (
          <EmptyState>
            <EmptyStateBody>No pods found in namespace {ns}.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default NamespaceView;
