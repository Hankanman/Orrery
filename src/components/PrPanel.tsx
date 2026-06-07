import { useEffect, useState } from "react";
import {
  Check,
  CheckCircle2,
  Clock,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  MinusCircle,
  XCircle,
} from "lucide-react";
import { ipc, isTauri, type MergeMethod, type PrDetail, type PrPanel as PrPanelData } from "@/lib/ipc";
import { MOCK_PR_PANELS } from "@/lib/mock-activity";
import { cn } from "@/lib/utils";

const CHECK_ICON = {
  success: { Icon: CheckCircle2, cls: "text-ok" },
  failure: { Icon: XCircle, cls: "text-danger" },
  pending: { Icon: Clock, cls: "text-warn" },
  neutral: { Icon: MinusCircle, cls: "text-muted-foreground" },
} as const;

const MERGE_LABEL: Record<MergeMethod, string> = {
  squash: "Squash & merge",
  rebase: "Rebase & merge",
  merge: "Create a merge commit",
};

/** Checks/review/merge panel for a repo's open PRs (GitHub). */
export function PrPanel({ slug }: { slug: string }) {
  const [panel, setPanel] = useState<PrPanelData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The PR number currently being merged/approved, to disable just its row.
  const [busy, setBusy] = useState<number | null>(null);

  const load = (refresh = false) => {
    setLoading(true);
    setError(null);
    if (!isTauri()) {
      setPanel(MOCK_PR_PANELS[slug] ?? { mergeMethods: [], prs: [] });
      setLoading(false);
      return Promise.resolve();
    }
    return ipc
      .prPanel(slug, refresh)
      .then(setPanel)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    let alive = true;
    setPanel(null);
    if (!isTauri()) {
      setPanel(MOCK_PR_PANELS[slug] ?? { mergeMethods: [], prs: [] });
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    ipc
      .prPanel(slug)
      .then((p) => alive && setPanel(p))
      .catch((e) => alive && setError(String(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [slug]);

  const doMerge = async (pr: PrDetail, method: MergeMethod) => {
    if (!isTauri() || busy !== null) return;
    setBusy(pr.number);
    setError(null);
    try {
      await ipc.mergePr(slug, pr.number, method);
      await load(true); // merged PR drops out of the open set
    } catch (e) {
      setError(`Couldn't merge #${pr.number}: ${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const doApprove = async (pr: PrDetail) => {
    if (!isTauri() || busy !== null) return;
    setBusy(pr.number);
    setError(null);
    try {
      await ipc.approvePr(slug, pr.number);
      await load(true);
    } catch (e) {
      setError(`Couldn't approve #${pr.number}: ${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  if (loading && !panel) return <p className="text-sm text-muted-foreground">Loading pull requests…</p>;
  if (error && !panel) return <p className="text-sm text-danger">{error}</p>;
  if (!panel || panel.prs.length === 0) {
    return <p className="text-sm text-muted-foreground">No open pull requests.</p>;
  }

  return (
    <div className="space-y-3">
      {error && <p className="text-sm text-danger">{error}</p>}
      {panel.prs.map((pr) => (
        <PrRow
          key={pr.number}
          pr={pr}
          mergeMethods={panel.mergeMethods}
          busy={busy === pr.number}
          disabled={busy !== null && busy !== pr.number}
          onMerge={doMerge}
          onApprove={doApprove}
        />
      ))}
    </div>
  );
}

function PrRow({
  pr,
  mergeMethods,
  busy,
  disabled,
  onMerge,
  onApprove,
}: {
  pr: PrDetail;
  mergeMethods: MergeMethod[];
  busy: boolean;
  disabled: boolean;
  onMerge: (pr: PrDetail, method: MergeMethod) => void;
  onApprove: (pr: PrDetail) => void;
}) {
  const [method, setMethod] = useState<MergeMethod>(mergeMethods[0] ?? "squash");

  const passed = pr.checks.filter((c) => c.state === "success").length;
  const failed = pr.checks.filter((c) => c.state === "failure").length;
  const running = pr.checks.filter((c) => c.state === "pending").length;
  const approvals = pr.reviews.filter((r) => r.state === "approved").length;
  const changesReq = pr.reviews.filter((r) => r.state === "changes_requested").length;

  // Merge is offered only when GitHub reports the branch as cleanly mergeable
  // and the PR isn't a draft. Branch protection (required checks / reviews) is
  // still enforced server-side — a blocked merge surfaces GitHub's reason.
  const canMerge = !pr.draft && pr.mergeable === "clean" && mergeMethods.length > 0;
  const mergeBlockReason = pr.draft
    ? "Draft PR"
    : pr.mergeable === "conflicting"
      ? "Has conflicts"
      : pr.mergeable === "unknown"
        ? "Mergeability unknown"
        : mergeMethods.length === 0
          ? "No merge method allowed"
          : "";

  return (
    <section className="rounded-md border border-border bg-background/40 p-3">
      <div className="flex items-start gap-2">
        <GitPullRequest className={cn("mt-0.5 size-4 shrink-0", pr.draft ? "text-muted-foreground" : "text-ok")} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <a
              href={pr.url}
              target="_blank"
              rel="noreferrer"
              className="min-w-0 truncate text-sm font-medium hover:underline"
            >
              {pr.title}
            </a>
            <ExternalLink className="size-3 shrink-0 text-muted-foreground" />
          </div>
          <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
            #{pr.number} · {pr.head} → {pr.base}
            {pr.author && ` · ${pr.author}`}
            {pr.draft && " · draft"}
          </p>
        </div>
      </div>

      {/* Checks + reviews summary */}
      <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
        {pr.checks.length > 0 ? (
          <span className="inline-flex items-center gap-1.5">
            {passed > 0 && (
              <span className="inline-flex items-center gap-0.5 text-ok">
                <CheckCircle2 className="size-3.5" />
                {passed}
              </span>
            )}
            {failed > 0 && (
              <span className="inline-flex items-center gap-0.5 text-danger">
                <XCircle className="size-3.5" />
                {failed}
              </span>
            )}
            {running > 0 && (
              <span className="inline-flex items-center gap-0.5 text-warn">
                <Clock className="size-3.5" />
                {running}
              </span>
            )}
            <span className="text-muted-foreground">checks</span>
          </span>
        ) : (
          <span className="text-muted-foreground">No checks</span>
        )}
        <span
          className={cn(
            changesReq > 0 ? "text-danger" : approvals > 0 ? "text-ok" : "text-muted-foreground",
          )}
        >
          {changesReq > 0
            ? `${changesReq} change${changesReq > 1 ? "s" : ""} requested`
            : approvals > 0
              ? `${approvals} approval${approvals > 1 ? "s" : ""}`
              : "No reviews"}
        </span>
        {pr.mergeable === "conflicting" && <span className="text-danger">Conflicts</span>}
      </div>

      {/* Per-check breakdown */}
      {pr.checks.length > 0 && (
        <ul className="mt-2 space-y-0.5">
          {pr.checks.map((c) => {
            const { Icon, cls } = CHECK_ICON[c.state];
            const row = (
              <span className="inline-flex items-center gap-1.5">
                <Icon className={cn("size-3.5 shrink-0", cls)} />
                <span className="truncate">{c.name}</span>
              </span>
            );
            return (
              <li key={`${c.name}-${c.url ?? ""}`} className="text-xs text-muted-foreground">
                {c.url ? (
                  <a href={c.url} target="_blank" rel="noreferrer" className="hover:underline">
                    {row}
                  </a>
                ) : (
                  row
                )}
              </li>
            );
          })}
        </ul>
      )}

      {/* Actions */}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        {mergeMethods.length > 1 && (
          <select
            className="rounded-md border border-border bg-background/60 px-2 py-1 text-xs outline-none focus:border-primary/50"
            value={method}
            onChange={(e) => setMethod(e.target.value as MergeMethod)}
            disabled={busy || disabled}
          >
            {mergeMethods.map((m) => (
              <option key={m} value={m}>
                {MERGE_LABEL[m]}
              </option>
            ))}
          </select>
        )}
        <button
          type="button"
          className="orr-cbtn ide"
          onClick={() => onMerge(pr, method)}
          disabled={!canMerge || busy || disabled}
          title={canMerge ? `${MERGE_LABEL[method]} #${pr.number}` : mergeBlockReason}
        >
          <GitMerge className="size-3.5" /> {busy ? "Merging…" : "Merge"}
        </button>
        <button
          type="button"
          className="orr-cbtn"
          onClick={() => onApprove(pr)}
          disabled={busy || disabled || pr.reviewDecision === "approved"}
          title={pr.reviewDecision === "approved" ? "Already approved" : `Approve #${pr.number}`}
        >
          <Check className="size-3.5" /> Approve
        </button>
      </div>
    </section>
  );
}
