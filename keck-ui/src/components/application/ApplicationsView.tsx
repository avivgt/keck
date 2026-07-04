// SPDX-License-Identifier: Apache-2.0

import * as React from "react";
import {
  Page,
  PageSection,
  Title,
  Spinner,
  EmptyState,
  EmptyStateBody,
  Label,
  Tabs,
  Tab,
  TabTitleText,
  Select,
  SelectOption,
  MenuToggle,
  MenuToggleElement,
} from "@patternfly/react-core";
import {
  Table,
  Thead,
  Tbody,
  Tr,
  Th,
  Td,
  ThProps,
} from "@patternfly/react-table";
import { api, GroupPower } from "../../utils/api";
import { formatWatts } from "../../utils/format";
import { usePolling } from "../../utils/usePolling";

type SortKey = "group_name" | "total_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "storage_watts" | "io_watts" | "pod_count";

const CATEGORIES = ["all", "application", "operator", "platform"] as const;
type Category = typeof CATEGORIES[number];

const TAB_LABELS: Record<Category, string> = {
  all: "All",
  application: "Applications",
  operator: "Operators",
  platform: "Cluster Operators",
};

const ApplicationsView: React.FC = () => {
  const [category, setCategory] = React.useState<Category>("all");
  const [groupBy, setGroupBy] = React.useState("application");
  const [sortBy, setSortBy] = React.useState<SortKey>("total_watts");
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("desc");
  const [groupByOpen, setGroupByOpen] = React.useState(false);

  const cat = category === "all" ? undefined : category;
  const { data: groupsData, loading } = usePolling(
    () => api.getApplications(groupBy, cat),
    [category, groupBy],
  );
  const groups = groupsData || [];

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

  const totalWatts = groups.reduce((sum, g) => sum + g.total_watts, 0);
  const totalPods = groups.reduce((sum, g) => sum + g.pod_count, 0);

  const sorted = [...groups].sort((a, b) => {
    const av = (a as any)[sortBy];
    const bv = (b as any)[sortBy];
    if (typeof av === "string") return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
    return sortDir === "asc" ? av - bv : bv - av;
  });

  const cols: SortKey[] = ["group_name", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "storage_watts", "io_watts", "pod_count"];
  const getSortParams = (key: SortKey): ThProps["sort"] => ({
    sortBy: { index: cols.indexOf(sortBy), direction: sortDir },
    onSort: (_e, _idx, dir) => { setSortBy(key); setSortDir(dir as "asc" | "desc"); },
    columnIndex: cols.indexOf(key),
  });

  const groupByOptions = [
    { value: "application", label: "Application" },
    { value: "workload", label: "Workload" },
    { value: "namespace", label: "Namespace" },
    { value: "label:app.kubernetes.io/name", label: "App Label" },
    { value: "label:app.kubernetes.io/part-of", label: "Part Of" },
    { value: "label:argocd.argoproj.io/instance", label: "ArgoCD App" },
  ];

  const categoryColor = (cat: string) => {
    if (cat === "platform") return "purple";
    if (cat === "operator") return "blue";
    return "green";
  };

  const categoryLabel = (cat: string) => {
    if (cat === "platform") return "cluster operator";
    return cat;
  };

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="xl">Applications</Title>
        <p style={{ marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
          {groups.length} groups, {totalPods} pods, {formatWatts(totalWatts)} total.
        </p>
      </PageSection>

      <PageSection>
        <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 16 }}>
          <Tabs activeKey={category} onSelect={(_e, key) => setCategory(String(key) as Category)}>
            <Tab eventKey="all" title={<TabTitleText>All</TabTitleText>} />
            <Tab eventKey="application" title={<TabTitleText>Applications</TabTitleText>} />
            <Tab eventKey="operator" title={<TabTitleText>Operators</TabTitleText>} />
            <Tab eventKey="platform" title={<TabTitleText>Cluster Operators</TabTitleText>} />
          </Tabs>
          <div style={{ marginLeft: "auto" }}>
            <Select
              isOpen={groupByOpen}
              onOpenChange={setGroupByOpen}
              onSelect={(_e, val) => { setGroupBy(val as string); setGroupByOpen(false); }}
              selected={groupBy}
              toggle={(toggleRef: React.Ref<MenuToggleElement>) => (
                <MenuToggle ref={toggleRef} onClick={() => setGroupByOpen(!groupByOpen)} isExpanded={groupByOpen}>
                  Group by: {groupByOptions.find(o => o.value === groupBy)?.label || groupBy}
                </MenuToggle>
              )}
            >
              {groupByOptions.map(opt => (
                <SelectOption key={opt.value} value={opt.value}>{opt.label}</SelectOption>
              ))}
            </Select>
          </div>
        </div>

        {sorted.length > 0 ? (
          <Table aria-label="Application power table" variant="compact">
            <Thead>
              <Tr>
                <Th sort={getSortParams("group_name")}>Name</Th>
                <Th>Kind</Th>
                <Th>Category</Th>
                <Th>Namespace</Th>
                <Th sort={getSortParams("total_watts")}>Total Power</Th>
                <Th sort={getSortParams("cpu_watts")}>CPU</Th>
                <Th sort={getSortParams("memory_watts")}>Memory</Th>
                <Th sort={getSortParams("gpu_watts")}>GPU</Th>
                <Th sort={getSortParams("storage_watts")}>Storage</Th>
                <Th sort={getSortParams("io_watts")}>Network</Th>
                <Th sort={getSortParams("pod_count")}>Pods</Th>
              </Tr>
            </Thead>
            <Tbody>
              {sorted.map((g) => (
                <Tr key={g.group_key}>
                  <Td style={{ fontWeight: 600 }}>{g.group_name}</Td>
                  <Td><Label style={{ fontSize: "11px" }}>{g.group_kind}</Label></Td>
                  <Td><Label color={categoryColor(g.category)} style={{ fontSize: "11px" }}>{categoryLabel(g.category)}</Label></Td>
                  <Td style={{ fontSize: "0.9em", color: "var(--pf-v6-global--Color--200)" }}>{g.namespace || "multiple"}</Td>
                  <Td style={{ fontWeight: 600 }}>{formatWatts(g.total_watts)}</Td>
                  <Td>{formatWatts(g.cpu_watts)}</Td>
                  <Td>{formatWatts(g.memory_watts)}</Td>
                  <Td>{formatWatts(g.gpu_watts)}</Td>
                  <Td>{formatWatts(g.storage_watts)}</Td>
                  <Td>{formatWatts(g.io_watts)}</Td>
                  <Td>{g.pod_count}</Td>
                </Tr>
              ))}
            </Tbody>
          </Table>
        ) : (
          <EmptyState titleText="No Data">
            <EmptyStateBody>No application power data available{category !== "all" ? ` for ${TAB_LABELS[category]}` : ""}.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default ApplicationsView;
