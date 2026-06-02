import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Bell, CircleDot, Eye, GitPullRequest, Inbox } from "lucide-react";
import { ipc, isTauri, type InboxItem, type NotificationItem } from "@/lib/ipc";
import { MOCK_INBOX, MOCK_NOTIFICATIONS } from "@/lib/mock-activity";
import { Spinner } from "@/components/Spinner";

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
  const [inbox, setInbox] = useState<InboxItem[] | null>(null);
  const [notes, setNotes] = useState<NotificationItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      setInbox(MOCK_INBOX);
      setNotes(MOCK_NOTIFICATIONS);
      return;
    }
    ipc.getInbox().then(setInbox).catch((e) => {
      setInbox([]);
      setError(String(e));
    });
    ipc.getNotifications().then(setNotes).catch(() => setNotes([]));
  }, []);

  const byKind = (kind: InboxItem["kind"]) => (inbox ?? []).filter((i) => i.kind === kind);

  const empty = inbox !== null && inbox.length === 0 && (notes ?? []).length === 0;

  return (
    <div className="orr-inbox">
      <header className="orr-settings-head">
        <h1>Inbox</h1>
        <p>Open pull requests, review requests, assigned issues, and notifications.</p>
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
      </div>
    </div>
  );
}
