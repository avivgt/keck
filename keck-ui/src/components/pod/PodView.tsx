// SPDX-License-Identifier: Apache-2.0

// Pod detail view — deepest drill-down level.
// Shows per-process and per-component power breakdown.

import * as React from "react";
import { useParams, Link } from "react-router-dom";
import {
  Page,
  PageSection,
  Title,
  Breadcrumb,
  BreadcrumbItem,
  Card,
  CardTitle,
  CardBody,
  Gallery,
  GalleryItem,
  Spinner,
  EmptyState,
  EmptyStateBody,
} from "@patternfly/react-core";
import { api, PodPower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

const PodView: React.FC = () => {
  const { uid } = useParams<{ uid: string }>();
  const [pod, setPod] = React.useState<PodPower | null>(null);
  const [loading, setLoading] = React.useState(true);

  React.useEffect(() => {
    if (!uid) return;
    // TODO: fetch pod detail from agent query API
    setLoading(false);
  }, [uid]);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

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
          <BreadcrumbItem isActive>Pod {uid?.slice(0, 8)}</BreadcrumbItem>
        </Breadcrumb>

        <Title headingLevel="h1" size="xl" style={{ marginTop: 16 }}>
          Pod Detail
        </Title>
        <p style={{ color: "var(--pf-v6-global--Color--200)" }}>
          Per-process power breakdown. Requires agent Full profile.
        </p>
      </PageSection>

      <PageSection>
        <EmptyState>
          <Title headingLevel="h2" size="lg">Process Detail</Title>
          <EmptyStateBody>
            Pod drill-down requires the keck-agent query API to be connected.
            This will show per-process power with per-core attribution detail.
          </EmptyStateBody>
        </EmptyState>
      </PageSection>
    </Page>
  );
};

export default PodView;
