import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, CloudDownload, Code, GitMerge, Loader2, Play, Scissors, Terminal, X } from "lucide-react";
import { ipc, isTauri, type BulkOp, type BulkProgress } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { cn } from "@/lib/utils";

interface FleetBarProps {
  selectedIds: Set<string>;
  visibleCount: number;
  allSelected: boolean;
  onSelectAllVisible: () => void;
  onClear: () => void;
}

/**
 * Fleet action bar: operate on the multi-selected repos at once. Streams
 * per-repo results from the backend `bulk_op` over `bulk-progress` events,
 * with a live count and a cancel button. Renders nothing when the selection is
 * empty and no run is in flight (kept mounted by the parent so an in-flight run
 * survives the selection being cleared).
 */
export function FleetBar({ selectedIds, visibleCount, allSelected, onSelectAllVisible, onClear }: FleetBarProps) {
  const { repos, openIde, refresh } = useRepos();
  const [running, setRunning] = useState(false);
  const [results, setResults] = useState<Map<string, BulkProgress>>(new Map());
  const [total, setTotal] = useState(0);
  const [command, setCommand] = useState("");
  const [showResults, setShowResults] = useState(false);
  const runId = useRef<string>("");

  // Persistent listeners for the component's lifetime — filtered to the active
  // run id, so a stale/cross run can't bleed into the current display.
  useEffect(() => {
    if (!isTauri()) return;
    const unlisteners: Array<() => void> = [];
    listen<BulkProgress>("bulk-progress", (e) => {
      if (e.payload.runId !== runId.current) return;
      setResults((m) => new Map(m).set(e.payload.id, e.payload));
    }).then((u) => unlisteners.push(u));
    listen<{ runId: string; cancelled: boolean }>("bulk-done", (e) => {
      if (e.payload.runId !== runId.current) return;
      setRunning(false);
      refresh(); // pick up new git state from fetch/pull/checkout
    }).then((u) => unlisteners.push(u));
    return () => unlisteners.forEach((u) => u());
  }, [refresh]);

  const selectedRepos = useMemo(() => repos.filter((r) => selectedIds.has(r.id)), [repos, selectedIds]);

  const run = async (op: BulkOp) => {
    if (!isTauri() || running || selectedIds.size === 0) return;
    const ids = [...selectedIds];
    const id = `${Date.now()}-${Math.round(Math.random() * 1e6)}`;
    runId.current = id;
    setResults(new Map());
    setTotal(ids.length);
    setShowResults(true);
    setRunning(true);
    try {
      await ipc.bulkOp(id, ids, op);
    } catch {
      setRunning(false); // bulk-done won't arrive if the invoke itself failed
    }
  };

  const cancel = () => {
    if (isTauri()) ipc.cancelBulk().catch(() => {});
  };

  // "Open all in IDE" is a pure frontend fan-out — no streaming/cancel needed.
  const openAllInIde = () => {
    selectedRepos.forEach((r) => openIde(r));
  };

  const runCommand = () => {
    const c = command.trim();
    if (c) run({ kind: "runCommand", command: c });
  };

  if (selectedIds.size === 0 && !running) return null;

  const done = results.size;
  const tally = { ok: 0, skipped: 0, error: 0 };
  for (const r of results.values()) tally[r.status]++;

  const nameOf = (id: string) => repos.find((r) => r.id === id)?.displayName ?? id.split("/").pop() ?? id;

  return (
    <div className="orr-fleetbar">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium">{selectedIds.size} selected</span>
        <button type="button" className="orr-sortpill" onClick={onSelectAllVisible} disabled={running}>
          {allSelected ? "Deselect all" : `Select all ${visibleCount}`}
        </button>

        <span className="mx-1 h-4 w-px bg-border" aria-hidden />

        <button type="button" className="orr-sortpill" onClick={() => run({ kind: "fetch" })} disabled={running}>
          <CloudDownload className="size-3.5" /> Fetch
        </button>
        <button type="button" className="orr-sortpill" onClick={() => run({ kind: "pull" })} disabled={running}>
          <GitMerge className="size-3.5" /> Pull
        </button>
        <button type="button" className="orr-sortpill" onClick={() => run({ kind: "stash" })} disabled={running}>
          <Scissors className="size-3.5" /> Stash
        </button>
        <button type="button" className="orr-sortpill" onClick={() => run({ kind: "checkoutDefault" })} disabled={running}>
          <GitMerge className="size-3.5" /> Checkout default
        </button>
        <button type="button" className="orr-sortpill" onClick={openAllInIde} disabled={running}>
          <Code className="size-3.5" /> Open in IDE
        </button>

        <form
          className="flex items-center gap-1"
          onSubmit={(e) => {
            e.preventDefault();
            runCommand();
          }}
        >
          <div className="flex items-center gap-1 rounded-md border border-border bg-background/60 px-2">
            <Terminal className="size-3.5 text-muted-foreground" />
            <input
              className="w-40 bg-transparent py-1 font-mono text-xs outline-none"
              placeholder="pnpm install"
              value={command}
              spellCheck={false}
              disabled={running}
              onChange={(e) => setCommand(e.target.value)}
            />
          </div>
          <button type="submit" className="orr-sortpill" disabled={running || !command.trim()} title="Run command in each repo">
            <Play className="size-3.5" /> Run
          </button>
        </form>

        <div className="ml-auto flex items-center gap-2">
          {running ? (
            <>
              <span className="flex items-center gap-1.5 text-sm text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" /> {done}/{total}
              </span>
              <button type="button" className="orr-sortpill" onClick={cancel}>
                <X className="size-3.5" /> Cancel
              </button>
            </>
          ) : results.size > 0 ? (
            <button
              type="button"
              className="orr-sortpill"
              onClick={() => setShowResults((s) => !s)}
              title="Toggle per-repo results"
            >
              <Check className="size-3.5 text-ok" /> {tally.ok}
              {tally.skipped > 0 && <span className="text-warn">· ⤼ {tally.skipped}</span>}
              {tally.error > 0 && <span className="text-danger">· ✕ {tally.error}</span>}
            </button>
          ) : null}
          {!running && (
            <button type="button" className="orr-iconbtn" aria-label="Clear selection" onClick={onClear}>
              <X className="size-4" />
            </button>
          )}
        </div>
      </div>

      {showResults && results.size > 0 && (
        <ul className="mt-2 max-h-40 space-y-0.5 overflow-y-auto border-t border-border/60 pt-2 text-xs">
          {[...results.values()].map((r) => (
            <li key={r.id} className="flex items-center gap-2">
              <span
                className={cn(
                  "shrink-0 font-medium",
                  r.status === "ok" && "text-ok",
                  r.status === "skipped" && "text-warn",
                  r.status === "error" && "text-danger",
                )}
              >
                {r.status === "ok" ? "✓" : r.status === "skipped" ? "⤼" : "✕"}
              </span>
              <span className="shrink-0 font-medium">{nameOf(r.id)}</span>
              <span className="min-w-0 flex-1 truncate font-mono text-muted-foreground">{r.detail}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
