// SPDX-License-Identifier: Apache-2.0

import * as React from "react";

const POLL_INTERVAL = 5000;

export function usePolling<T>(
  fetcher: () => Promise<T>,
  deps: React.DependencyList = [],
): { data: T | null; loading: boolean; error: string | null } {
  const [data, setData] = React.useState<T | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const poll = () => {
      if (!active) return;
      if (document.visibilityState === "hidden") {
        timer = setTimeout(poll, POLL_INTERVAL);
        return;
      }
      fetcher()
        .then((result) => { if (active) { setData(result); setError(null); } })
        .catch((e) => { if (active) setError(e.message); })
        .finally(() => {
          if (active) {
            setLoading(false);
            timer = setTimeout(poll, POLL_INTERVAL);
          }
        });
    };

    poll();

    const onVisibility = () => {
      if (document.visibilityState === "visible" && active) {
        if (timer) clearTimeout(timer);
        poll();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);

    return () => {
      active = false;
      if (timer) clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, deps);

  return { data, loading, error };
}
