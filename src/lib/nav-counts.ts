import { useEffect, useMemo, useState } from "react";
import { ipc, isTauri } from "@/lib/ipc";
import { MOCK_INBOX, MOCK_PRUNABLE } from "@/lib/mock-activity";
import { needsAttention } from "@/lib/repo-filter";
import { useRepos } from "@/lib/repos-context";

export interface NavCounts {
  /** Repos with uncommitted work, unpushed/behind commits, or that are stale. */
  attention: number;
  /** Open PRs / review requests / assigned issues across your hosts. */
  inbox: number;
  /** Branches safe to prune (merged or gone upstream) across all repos. */
  prunable: number;
}

const branchTotal = (groups: { branches: unknown[] }[]) =>
  groups.reduce((n, g) => n + g.branches.length, 0);

/**
 * Counts for the sidebar nav badges. `attention` is derived from the in-memory
 * repo list (free); `inbox` and `prunable` are fetched lazily once repos are
 * loaded and refreshed after each scan. Everything degrades to 0 (badge hidden)
 * when unavailable, so a missing token or git error never breaks the rail. The
 * prunable scan is deferred a beat so it doesn't compete with first paint.
 */
export function useNavCounts(): NavCounts {
  const { repos, lastScan } = useRepos();
  const attention = useMemo(() => repos.filter(needsAttention).length, [repos]);

  const [inbox, setInbox] = useState(0);
  const [prunable, setPrunable] = useState(0);

  const ids = useMemo(() => repos.map((r) => r.id), [repos]);
  const idsKey = ids.join("|");

  useEffect(() => {
    if (!isTauri()) {
      setInbox(MOCK_INBOX.length);
      setPrunable(branchTotal(MOCK_PRUNABLE));
      return;
    }
    let alive = true;
    ipc
      .getInbox()
      .then((items) => alive && setInbox(items.length))
      .catch(() => {});

    let timer: ReturnType<typeof setTimeout> | undefined;
    if (ids.length) {
      timer = setTimeout(() => {
        ipc
          .prunableBranches(ids)
          .then((groups) => alive && setPrunable(branchTotal(groups)))
          .catch(() => {});
      }, 600);
    }
    return () => {
      alive = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey, lastScan]);

  return { attention, inbox, prunable };
}
