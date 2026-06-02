import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Rss, Tag } from "lucide-react";
import { ipc, isTauri, type ReleaseItem } from "@/lib/ipc";
import { HostIcon } from "@/components/HostIcon";
import { Spinner } from "@/components/Spinner";
import { VirtualList } from "@/components/VirtualList";
import { timeAgo } from "@/lib/format";

function open(url: string) {
  if (isTauri()) openUrl(url).catch(() => {});
  else window.open(url, "_blank");
}

/** Release feed — new releases across the repos you've starred, newest first. */
export function FeedView() {
  const [items, setItems] = useState<ReleaseItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      setItems([]);
      return;
    }
    ipc
      .releaseFeed()
      .then(setItems)
      .catch((e) => {
        setItems([]);
        setError(String(e));
      });
  }, []);

  return (
    <div className="orr-feed">
      <header className="orr-feed-head">
        <h1>Feed</h1>
        <p>New releases from the repos you've starred.</p>
      </header>

      {items === null ? (
        <div className="orr-empty">
          <Spinner size={32} />
          <p className="s">Loading releases…</p>
        </div>
      ) : error ? (
        <div className="orr-empty">
          <Rss className="size-8 opacity-60" />
          <p className="t">Couldn’t load the feed</p>
          <p className="s">{error}</p>
        </div>
      ) : items.length === 0 ? (
        <div className="orr-empty">
          <Rss className="size-8 opacity-60" />
          <p className="t">Nothing new</p>
          <p className="s">Star some repos on GitHub (and connect it in Settings) to see their releases here.</p>
        </div>
      ) : (
        <VirtualList
          items={items}
          getKey={(r) => `${r.repo}@${r.tag}`}
          estimateSize={104}
          gap={10}
          className="orr-feed-list"
          renderItem={(r) => (
            <button type="button" className="orr-feed-item" onClick={() => open(r.url)}>
              <div className="row">
                <HostIcon host={r.host} className="size-3.5 opacity-70" />
                <span className="repo">{r.repo}</span>
                <span className="ver">
                  <Tag className="size-3" /> {r.tag}
                </span>
                {r.prerelease && <span className="pre">pre-release</span>}
                <span className="time">{timeAgo(r.publishedAt)}</span>
              </div>
              {r.name && r.name !== r.tag && <div className="name">{r.name}</div>}
              {r.body && <p className="body">{r.body}</p>}
            </button>
          )}
        />
      )}
    </div>
  );
}
