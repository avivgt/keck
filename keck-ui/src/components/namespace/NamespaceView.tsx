// SPDX-License-Identifier: Apache-2.0

// Pod-level power for a specific namespace. Drill-down from ClusterOverview.

import * as React from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
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

const NamespaceView: React.FC = () => {
  const { ns } = useParams<{ ns: string }>();
  const [pods, setPods] = React.useState<PodPower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const navigate = useNavigate();

  React.useEffect(() => {
    if (!ns) return;
    const fetchData = () => {
      api.getNamespacePods(ns)
        .then(setPods)
        .finally(() => setLoading(false));
    };
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [ns]);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
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
                <Tr
                  key={pod.pod_uid}
                  isClickable
                  onRowClick={() => navigate(`/power-management/pods/${pod.pod_uid}`)}
                >
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
