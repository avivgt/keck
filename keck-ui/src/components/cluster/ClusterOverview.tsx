// SPDX-License-Identifier: Apache-2.0

// Namespace power breakdown — shows all namespaces with power consumption.
// Click a namespace to drill down to pods.

import * as React from "react";
import { useHistory } from "react-router-dom";
import {
  Page,
  PageSection,
  Title,
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
import { api, NamespacePower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

const ClusterOverview: React.FC = () => {
  const [namespaces, setNamespaces] = React.useState<NamespacePower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const history = useHistory();

  React.useEffect(() => {
    const fetchData = () => {
      api.getNamespaces()
        .then(setNamespaces)
        .finally(() => setLoading(false));
    };
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

  const totalWatts = namespaces.reduce((sum, ns) => sum + ns.total_watts, 0);

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="xl">
          Power by Namespace
        </Title>
        <p style={{ marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
          {namespaces.length} namespaces, {formatWatts(totalWatts)} total.
          Click a namespace to see pod-level detail.
        </p>
      </PageSection>

      <PageSection>
        {namespaces.length > 0 ? (
          <Table aria-label="Namespace power table" variant="compact">
            <Thead>
              <Tr>
                <Th>Namespace</Th>
                <Th>Total Power</Th>
                <Th>CPU</Th>
                <Th>Memory</Th>
                <Th>GPU</Th>
                <Th>Pods</Th>
              </Tr>
            </Thead>
            <Tbody>
              {namespaces.map((ns) => (
                <Tr
                  key={ns.namespace}
                  isClickable
                  onRowClick={() => history.push(`/power-management/namespaces/${ns.namespace}`)}
                >
                  <Td>{ns.namespace}</Td>
                  <Td>{formatWatts(ns.total_watts)}</Td>
                  <Td>{formatWatts(ns.cpu_watts)}</Td>
                  <Td>{formatWatts(ns.memory_watts)}</Td>
                  <Td>{formatWatts(ns.gpu_watts)}</Td>
                  <Td>{ns.pod_count}</Td>
                </Tr>
              ))}
            </Tbody>
          </Table>
        ) : (
          <EmptyState>
            <EmptyStateBody>No namespace power data available.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default ClusterOverview;
