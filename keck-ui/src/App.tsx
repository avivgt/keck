// SPDX-License-Identifier: Apache-2.0

import { Routes, Route, NavLink } from "react-router-dom";
import { FleetOverview } from "./components/fleet/FleetOverview";
import { ClusterOverview } from "./components/cluster/ClusterOverview";
import { NamespaceView } from "./components/namespace/NamespaceView";
import { PodView } from "./components/pod/PodView";
import "./App.css";

export function App() {
  return (
    <div className="app">
      <nav className="sidebar">
        <div className="logo">
          <h1>Keck</h1>
          <span className="tagline">Power Observability</span>
        </div>

        <ul className="nav-links">
          <li>
            <NavLink to="/">Fleet</NavLink>
          </li>
          <li>
            <NavLink to="/cluster">Cluster</NavLink>
          </li>
          <li>
            <NavLink to="/nodes">Nodes</NavLink>
          </li>
        </ul>
      </nav>

      <main className="content">
        <Routes>
          <Route path="/" element={<FleetOverview />} />
          <Route path="/cluster" element={<ClusterOverview />} />
          <Route path="/namespaces/:namespace" element={<NamespaceView />} />
          <Route path="/pods/:uid" element={<PodView />} />
        </Routes>
      </main>
    </div>
  );
}
