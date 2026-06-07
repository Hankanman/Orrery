import { useEffect, useMemo, useState } from "react";
import { ipc, isTauri, type FeedItem } from "@/lib/ipc";
import { useLastVisit } from "@/lib/last-visit";
import { MOCK_AGENT_SESSIONS, MOCK_FEED, MOCK_INBOX, MOCK_PRUNABLE } from "@/lib/mock-activity";
import { needsAttention } from "@/lib/repo-filter";
import { useRepos } from "@/lib/repos-context";

export interface NavCounts {
  /** Repos with uncommitted work, unpushed/behind commits, or that are stale. */
  attention: number;
  /** Open PRs / review requests / assigned issues across your hosts. */
  inbox: number;
  /** Feed items newer than your last visit to the Feed (0 until first visit). */
  feedNew: number;
  /** Branches safe to prune (merged or gone upstream) across all repos. */
  prunable: number;
  /** Repos with a running terminal-agent session. */
  agents: number;
}

const branchTotal = (groups: { branches: unknown[] }[]) =>
  groups.reduce((n, g) => n + g.branches.length, 0);

/** Count feed items newer than the last visit — but nothing until the first
 *  visit establishes a baseline, so a fresh install doesn't flag everything. */
const newSince = (items: FeedItem[], since: number) =>
  since === 0 ? 0 : items.filter((i) => i.timestamp > since).length;

/**
 * Counts for the sidebar nav badges. `attention` is derived from the in-memory
 * repo list (free); `inbox`, `feedNew`, and `prunable` are fetched lazily once
 * repos are loaded and refreshed after each scan. Everything degrades to 0
 * (badge hidden) when unavailable, so a missing token or git error never breaks
 * the rail. The feed and prunable lookups are deferred a beat so they don't
 * compete with first paint.
 */
export function useNavCounts(): NavCounts {
  const { repos, lastScan, activeAgents } = useRepos();
  const feedSeen = useLastVisit("feed");
  const attention = useMemo(() => repos.filter(needsAttention).length, [repos]);

  const [inbox, setInbox] = useState(0);
  const [prunable, setPrunable] = useState(0);
  const [feed, setFeed] = useState<FeedItem[]>([]);

  const ids = useMemo(() => repos.map((r) => r.id), [repos]);
  const idsKey = ids.join("|");

  useEffect(() => {
    if (!isTauri()) {
      setInbox(MOCK_INBOX.length);
      setPrunable(branchTotal(MOCK_PRUNABLE));
      setFeed(MOCK_FEED);
      return;
    }
    let alive = true;
    ipc
      .getInbox()
      .then((items) => alive && setInbox(items.length))
      .catch(() => {});

    const timer = setTimeout(() => {
      if (!alive) return;
      ipc
        .getFeed(false)
        .then((items) => alive && setFeed(items))
        .catch(() => {});
      if (ids.length) {
        ipc
          .prunableBranches(ids)
          .then((groups) => alive && setPrunable(branchTotal(groups)))
          .catch(() => {});
      }
    }, 600);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey, lastScan]);

  const feedNew = useMemo(() => newSince(feed, feedSeen), [feed, feedSeen]);
  const agents = isTauri() ? activeAgents.length : MOCK_AGENT_SESSIONS.length;

  return { attention, inbox, feedNew, prunable, agents };
}
