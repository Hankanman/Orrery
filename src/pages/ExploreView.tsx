import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Compass, DownloadCloud, Star } from "lucide-react";
import { ipc, isTauri, type RemoteRepo } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { HostIcon } from "@/components/HostIcon";
import { Spinner } from "@/components/Spinner";
import { VirtualGrid } from "@/components/VirtualGrid";
import { formatStars, languageColor } from "@/lib/format";

function open(url: string) {
  if (isTauri()) openUrl(url).catch(() => {});
  else window.open(url, "_blank");
}

/** Explore — browse the repos you've starred and clone them into a workspace root. */
export function ExploreView() {
  const { refresh } = useRepos();
  const [starred, setStarred] = useState<RemoteRepo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cloning, setCloning] = useState<string | null>(null);
  const [cloned, setCloned] = useState<Set<string>>(new Set());
  const [root, setRoot] = useState("~/dev");

  useEffect(() => {
    if (!isTauri()) {
      setStarred([]);
      return;
    }
    ipc.getConfig().then((c) => setRoot(c.roots[0] ?? "~/dev")).catch(() => {});
    ipc.listStarred().then(setStarred).catch((e) => {
      setStarred([]);
      setError(String(e));
    });
  }, []);

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

  return (
    <div className="orr-feed">
      <header className="orr-feed-head">
        <h1>Explore</h1>
        <p>
          Repos you've starred. {root && (
            <>
              Clones land in <code>{root}</code>.
            </>
          )}
        </p>
        {error && <p className="mt-2 text-sm text-danger">{error}</p>}
      </header>

      {starred === null ? (
        <div className="orr-empty">
          <Spinner size={32} />
          <p className="s">Loading your stars…</p>
        </div>
      ) : starred.length === 0 ? (
        <div className="orr-empty">
          <Compass className="size-8 opacity-60" />
          <p className="t">No starred repos</p>
          <p className="s">Star repos on GitHub (and connect it in Settings) to browse and clone them here.</p>
        </div>
      ) : (
        <VirtualGrid
          items={starred}
          minColWidth={260}
          colGap={12}
          rowGap={12}
          estimateRow={120}
          className="orr-explore-grid"
          renderItem={(r) => (
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
          )}
        />
      )}
    </div>
  );
}
