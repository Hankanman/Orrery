import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FolderPlus, GitFork, Globe, RefreshCw, Rss, Star, Tag } from "lucide-react";
import { ipc, isTauri, type FeedItem } from "@/lib/ipc";
import { MOCK_FEED } from "@/lib/mock-activity";
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
  const [items, setItems] = useState<FeedItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
      ) : items.length === 0 ? (
        <div className="orr-empty">
          <Rss className="size-8 opacity-60" />
          <p className="t">Nothing new</p>
          <p className="s">Star repos and follow people on GitHub (and connect it in Settings) to fill your feed.</p>
        </div>
      ) : (
        <VirtualList
          items={items}
          getKey={(r) => `${r.url}:${r.kind}:${r.timestamp}`}
          estimateSize={96}
          gap={10}
          className="orr-feed-list"
          renderItem={(r) => {
            const Icon = KIND_ICON[r.kind];
            return (
              <button type="button" className="orr-feed-item" onClick={() => open(r.url)}>
                <div className="row">
                  <Icon className="size-3.5 shrink-0 opacity-70" />
                  <span className="repo">{r.repo}</span>
                  {r.kind === "release" && r.tag && (
                    <span className="ver">
                      <Tag className="size-3" /> {r.tag}
                    </span>
                  )}
                  {r.prerelease && <span className="pre">pre-release</span>}
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
