import { useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FolderPlus, GitFork, Globe, RefreshCw, Rss, Sparkle, Star, Tag } from "lucide-react";
import { ipc, isTauri, type FeedItem } from "@/lib/ipc";
import { lastVisit, markVisited } from "@/lib/last-visit";
import { MOCK_FEED } from "@/lib/mock-activity";
import { useSidebarSlot } from "@/lib/sidebar-slot";
import { Spinner } from "@/components/Spinner";
import { VirtualList } from "@/components/VirtualList";
import { timeAgo } from "@/lib/format";
import { cn } from "@/lib/utils";

function open(url: string) {
  if (isTauri()) openUrl(url).catch(() => {});
  else window.open(url, "_blank");
}

const KIND_ICON = {
  release: Tag,
  starred: Star,
  created: FolderPlus,
  forked: GitFork,
  public: Globe,
} as const;

const FEED_KINDS: { kind: FeedItem["kind"]; label: string }[] = [
  { kind: "release", label: "Releases" },
  { kind: "starred", label: "Stars" },
  { kind: "created", label: "New repos" },
  { kind: "forked", label: "Forks" },
  { kind: "public", label: "Open-sourced" },
];

type Filter = "all" | "new" | FeedItem["kind"];

// Last feed we successfully loaded, persisted so a return visit paints instantly
// instead of showing a spinner while the (cached or live) fetch comes back.
const SNAPSHOT_KEY = "orr.feed.snapshot";

function readSnapshot(): FeedItem[] | null {
  try {
    const raw = localStorage.getItem(SNAPSHOT_KEY);
    const parsed = raw ? (JSON.parse(raw) as FeedItem[]) : null;
    return Array.isArray(parsed) && parsed.length > 0 ? parsed : null;
  } catch {
    return null;
  }
}

function writeSnapshot(items: FeedItem[]) {
  try {
    localStorage.setItem(SNAPSHOT_KEY, JSON.stringify(items));
  } catch {
    /* quota or serialization failure — the snapshot is a nicety, not load-bearing */
  }
}

function actionText(r: FeedItem): string {
  const who = r.actor ?? "Someone";
  switch (r.kind) {
    case "release":
      return r.actor ? `${r.actor} released ${r.title || r.tag}` : r.title && r.title !== r.tag ? r.title : "New release";
    case "starred":
      return `${who} starred this`;
    case "created":
      return `${who} created this repository`;
    case "forked":
      return `${who} forked this`;
    case "public":
      return `${who} open-sourced this`;
  }
}

/** Feed — releases from repos you've starred + activity from people you follow. */
export function FeedView() {
  const [items, setItems] = useState<FeedItem[] | null>(() => readSnapshot());
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");

  // Snapshot the last visit before we stamp a new one, so items that arrived
  // since you last looked stay highlighted for the duration of this visit.
  const seenRef = useRef(lastVisit("feed"));
  const isNew = (r: FeedItem) => seenRef.current !== 0 && r.timestamp > seenRef.current;

  const load = (refresh = false) => {
    if (!isTauri()) {
      setItems(MOCK_FEED);
      return;
    }
    if (refresh) setRefreshing(true);
    ipc
      .getFeed(refresh)
      .then((f) => {
        setItems(f);
        writeSnapshot(f);
        setError(null);
      })
      .catch((e) => {
        setItems((prev) => prev ?? []);
        setError(String(e));
      })
      .finally(() => setRefreshing(false));
  };

  useEffect(() => {
    load(false);
    markVisited("feed"); // viewing the feed clears its "new" badge
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const all = items ?? [];
  const newCount = useMemo(() => all.filter(isNew).length, [all]);

  const visible = useMemo(() => {
    if (filter === "all") return all;
    if (filter === "new") return all.filter(isNew);
    return all.filter((r) => r.kind === filter);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [all, filter]);

  // Sidebar: filter the feed by type (and a "new since last visit" view).
  useSidebarSlot(
    useMemo(() => {
      const item = (f: Filter, label: string, Icon: typeof Tag, count: number, accent = false) => (
        <button
          key={f}
          type="button"
          className={cn("orr-sb-item", filter === f && "active")}
          onClick={() => setFilter(f)}
        >
          <Icon className="size-4" />
          <span className="nm">{label}</span>
          {count > 0 && <span className={accent ? "orr-sb-badge" : "count"}>{count}</span>}
        </button>
      );
      return (
        <div className="orr-sb-sec">
          <div className="orr-sb-lead">Filter</div>
          {item("all", "All activity", Rss, all.length)}
          {newCount > 0 && item("new", "New since last visit", Sparkle, newCount, true)}
          {FEED_KINDS.map(({ kind, label }) =>
            item(kind, label, KIND_ICON[kind], all.filter((r) => r.kind === kind).length),
          )}
        </div>
      );
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [filter, all, newCount]),
  );

  return (
    <div className="orr-feed">
      <header className="orr-feed-head">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <h1>Feed</h1>
            <p>Releases and activity from the repos and people you follow.</p>
          </div>
          <button
            type="button"
            className="orr-iconbtn"
            title="Refresh feed"
            aria-label="Refresh feed"
            onClick={() => load(true)}
            disabled={refreshing}
          >
            <RefreshCw className={cn("size-4", refreshing && "animate-spin")} />
          </button>
        </div>
      </header>

      {items === null ? (
        <div className="orr-empty">
          <Spinner size={32} />
          <p className="s">Loading your feed…</p>
        </div>
      ) : error && items.length === 0 ? (
        <div className="orr-empty">
          <Rss className="size-8 opacity-60" />
          <p className="t">Couldn’t load the feed</p>
          <p className="s">{error}</p>
        </div>
      ) : visible.length === 0 ? (
        <div className="orr-empty">
          <Rss className="size-8 opacity-60" />
          <p className="t">{filter === "all" ? "Nothing new" : "Nothing here"}</p>
          <p className="s">
            {filter === "all"
              ? "Star repos and follow people on GitHub (and connect it in Settings) to fill your feed."
              : "No activity matches this filter."}
          </p>
        </div>
      ) : (
        <VirtualList
          items={visible}
          getKey={(r) => `${r.url}:${r.kind}:${r.timestamp}`}
          estimateSize={96}
          gap={10}
          className="orr-feed-list"
          renderItem={(r) => {
            const Icon = KIND_ICON[r.kind];
            return (
              <button type="button" className={cn("orr-feed-item", isNew(r) && "new")} onClick={() => open(r.url)}>
                <div className="row">
                  <Icon className="size-3.5 shrink-0 opacity-70" />
                  <span className="repo">{r.repo}</span>
                  {r.kind === "release" && r.tag && (
                    <span className="ver">
                      <Tag className="size-3" /> {r.tag}
                    </span>
                  )}
                  {r.prerelease && <span className="pre">pre-release</span>}
                  {isNew(r) && <span className="nu">new</span>}
                  <span className="time">{timeAgo(r.timestamp)}</span>
                </div>
                <div className="action">{actionText(r)}</div>
                {r.detail && <p className="body">{r.detail}</p>}
              </button>
            );
          }}
        />
      )}
    </div>
  );
}
