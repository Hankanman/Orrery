import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Bell, CircleDot, DownloadCloud, Eye, GitPullRequest, Inbox, Star } from "lucide-react";
import { ipc, isTauri, type InboxItem, type NotificationItem, type RemoteRepo } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { HostIcon } from "@/components/HostIcon";
import { Spinner } from "@/components/Spinner";
import { formatStars, languageColor } from "@/lib/format";

function open(url: string) {
  if (isTauri()) openUrl(url).catch(() => {});
  else window.open(url, "_blank");
}

const KIND_META: Record<InboxItem["kind"], { label: string; icon: typeof GitPullRequest }> = {
  pr: { label: "My pull requests", icon: GitPullRequest },
  review: { label: "Awaiting your review", icon: Eye },
  issue: { label: "Assigned issues", icon: CircleDot },
};

function ItemRow({ item }: { item: InboxItem }) {
  const Icon = KIND_META[item.kind].icon;
  return (
    <button type="button" className="orr-inbox-row" onClick={() => open(item.url)}>
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate">{item.title}</span>
      {item.draft && <span className="orr-tag">draft</span>}
      <span className="shrink-0 font-mono text-xs text-muted-foreground">
        {item.repo}#{item.number}
      </span>
    </button>
  );
}

export function InboxView() {
  const { refresh } = useRepos();
  const [inbox, setInbox] = useState<InboxItem[] | null>(null);
  const [notes, setNotes] = useState<NotificationItem[] | null>(null);
  const [starred, setStarred] = useState<RemoteRepo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cloning, setCloning] = useState<string | null>(null);
  const [cloned, setCloned] = useState<Set<string>>(new Set());
  const [root, setRoot] = useState("~/dev");

  useEffect(() => {
    if (!isTauri()) {
      setInbox([]);
      setNotes([]);
      setStarred([]);
      return;
    }
    ipc.getConfig().then((c) => setRoot(c.roots[0] ?? "~/dev")).catch(() => {});
    ipc.getInbox().then(setInbox).catch((e) => {
      setInbox([]);
      setError(String(e));
    });
    ipc.getNotifications().then(setNotes).catch(() => setNotes([]));
    ipc.listStarred().then(setStarred).catch(() => setStarred([]));
  }, []);

  const byKind = (kind: InboxItem["kind"]) => (inbox ?? []).filter((i) => i.kind === kind);

  const clone = async (r: RemoteRepo) => {
    setCloning(r.slug);
    setError(null);
    try {
      await ipc.cloneRepo(r.cloneUrl, root);
      setCloned((prev) => new Set(prev).add(r.slug));
      refresh();
    } catch (e) {
      const msg = String(e);
      // "already exists" means it's already on disk — treat as cloned, not an error.
      if (msg.includes("already exists")) setCloned((prev) => new Set(prev).add(r.slug));
      else setError(msg);
    } finally {
      setCloning(null);
    }
  };

  const empty = inbox !== null && inbox.length === 0 && (notes ?? []).length === 0;

  return (
    <div className="orr-inbox">
      <header className="orr-settings-head">
        <h1>Inbox</h1>
        <p>Open work across your hosts. {root && <>Clones land in <code>{root}</code>.</>}</p>
      </header>

      {error && <p className="mt-3 text-sm text-danger">{error}</p>}

      <div className="orr-settings-body">
        {(["pr", "review", "issue"] as InboxItem["kind"][]).map((kind) => {
          const items = byKind(kind);
          if (items.length === 0) return null;
          const meta = KIND_META[kind];
          return (
            <section key={kind}>
              <h2 className="orr-inbox-head">
                <meta.icon className="size-4" /> {meta.label} <span className="count">{items.length}</span>
              </h2>
              <div className="orr-inbox-list">
                {items.map((i) => (
                  <ItemRow key={`${i.repo}#${i.number}-${i.kind}`} item={i} />
                ))}
              </div>
            </section>
          );
        })}

        {notes && notes.length > 0 && (
          <section>
            <h2 className="orr-inbox-head">
              <Bell className="size-4" /> Notifications <span className="count">{notes.length}</span>
            </h2>
            <div className="orr-inbox-list">
              {notes.map((n, i) => (
                <button
                  key={i}
                  type="button"
                  className="orr-inbox-row"
                  onClick={() => open(`https://github.com/${n.repo}`)}
                >
                  <Bell className="size-4 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 flex-1 truncate">{n.title}</span>
                  <span className="orr-tag">{n.reason.replace(/_/g, " ")}</span>
                  <span className="shrink-0 font-mono text-xs text-muted-foreground">{n.repo}</span>
                </button>
              ))}
            </div>
          </section>
        )}

        {empty && (
          <div className="orr-empty">
            <Inbox className="size-8 opacity-60" />
            <p className="t">Inbox zero</p>
            <p className="s">No open PRs, reviews, issues, or notifications.</p>
          </div>
        )}

        {inbox === null && (
          <div className="orr-empty">
            <Spinner size={32} />
            <p className="s">Loading… (connect GitHub in settings if this stays empty)</p>
          </div>
        )}

        {starred && starred.length > 0 && (
          <section>
            <h2 className="orr-inbox-head">
              <Star className="size-4" /> Starred <span className="count">{starred.length}</span>
            </h2>
            <div className="orr-star-grid">
              {starred.map((r) => (
                <div key={r.slug} className="orr-star-card">
                  <div className="flex items-center gap-2">
                    <HostIcon host={r.host} className="size-3.5 opacity-70" />
                    <button
                      type="button"
                      className="truncate font-medium hover:underline"
                      onClick={() => open(`https://${r.host === "gitlab" ? "gitlab.com" : "github.com"}/${r.slug}`)}
                    >
                      {r.slug}
                    </button>
                  </div>
                  {r.description && <p className="mt-1 line-clamp-2 text-xs text-muted-foreground">{r.description}</p>}
                  <div className="mt-2 flex items-center gap-3 text-xs text-muted-foreground">
                    {r.language && (
                      <span className="flex items-center gap-1">
                        <span className="size-2 rounded-full" style={{ background: languageColor(r.language) }} />
                        {r.language}
                      </span>
                    )}
                    <span className="flex items-center gap-1">
                      <Star className="size-3" />
                      {formatStars(r.stars)}
                    </span>
                    <button
                      type="button"
                      className="orr-cbtn ml-auto"
                      disabled={cloning === r.slug || cloned.has(r.slug)}
                      onClick={() => clone(r)}
                    >
                      <DownloadCloud className="size-3.5" />
                      {cloned.has(r.slug) ? "Cloned" : cloning === r.slug ? "Cloning…" : "Clone"}
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
