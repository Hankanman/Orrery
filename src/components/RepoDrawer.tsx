import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Code, FolderOpen, GitBranch, Plus, Scissors, Sparkles, SquareTerminal, Tag, Trash2, X } from "lucide-react";
import type { Repo } from "@/types";
import { ipc, isTauri, type BranchInfo, type CommitInfo, type WorktreeInfo } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { setRepoTags, useRepoTags } from "@/lib/repo-tags";
import { BrandIcon } from "@/components/BrandIcon";
import { HostIcon } from "@/components/HostIcon";
import { NotesPanel } from "@/components/NotesPanel";
import { PrPanel } from "@/components/PrPanel";
import { VirtualList } from "@/components/VirtualList";
import { timeAgo } from "@/lib/format";
import { cn } from "@/lib/utils";

type Tab = "overview" | "changes" | "pr" | "notes" | "readme";

const TAB_LABEL: Record<Tab, string> = {
  overview: "Overview",
  changes: "Changes",
  pr: "PRs",
  notes: "Notes",
  readme: "Readme",
};

export function RepoDrawer({ repo, onClose }: { repo: Repo | null; onClose: () => void }) {
  const { refresh, openIde, openAgent, aiReady, ideBrand, ideName, agentBrand, agentName } = useRepos();
  const [tab, setTab] = useState<Tab>("overview");
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [newWt, setNewWt] = useState("");
  const [newTag, setNewTag] = useState("");
  const tagMap = useRepoTags();
  const [log, setLog] = useState<CommitInfo[]>([]);
  const [diff, setDiff] = useState("");
  const [readme, setReadme] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [commitMsg, setCommitMsg] = useState("");
  const [changelog, setChangelog] = useState("");
  const [aiBusy, setAiBusy] = useState(false);

  const id = repo?.id;

  useEffect(() => {
    if (!id || !isTauri()) return;
    let alive = true;
    setTab("overview");
    setBranches([]);
    setWorktrees([]);
    setLog([]);
    setDiff("");
    setReadme(null);
    setCommitMsg("");
    setChangelog("");
    setError(null);
    ipc.listBranches(id).then((b) => alive && setBranches(b)).catch(() => {});
    ipc.listWorktrees(id).then((w) => alive && setWorktrees(w)).catch(() => {});
    ipc.repoLog(id, 15).then((l) => alive && setLog(l)).catch(() => {});
    ipc.repoStagedDiff(id).then((d) => alive && setDiff(d)).catch(() => {});
    ipc.repoReadme(id).then((r) => alive && setReadme(r)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [id]);

  // The PRs tab is GitHub-only; fall back to overview if the current repo
  // can't show it (e.g. after switching to a repo with no GitHub remote).
  const showPr = repo?.host === "github" && !!repo?.slug;
  useEffect(() => {
    if (tab === "pr" && !showPr) setTab("overview");
  }, [tab, showPr]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  if (!repo) return null;

  const reload = async () => {
    if (!id) return;
    const snap = id;
    const next = await ipc.listBranches(id).catch(() => branches);
    if (repo?.id !== snap) return; // drawer moved to a different repo mid-call
    setBranches(next);
    refresh();
  };

  const doSwitch = async (name: string) => {
    if (!id || busy) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.switchBranch(id, name);
      await reload();
    } catch (e) {
      // libgit2 refuses a checkout that would clobber uncommitted work.
      setError(`Couldn't switch branch: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const doPrune = async () => {
    if (!id || busy) return;
    const victims = branches.filter((b) => !b.isHead && (b.merged || b.gone)).map((b) => b.name);
    if (victims.length === 0) return;
    if (!window.confirm(`Delete ${victims.length} branch(es)?\n\n${victims.join("\n")}`)) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.pruneBranches(id);
      await reload();
    } catch (e) {
      setError(`Couldn't prune: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const removeWt = async (name: string) => {
    if (!id) return;
    if (!window.confirm(`Remove worktree "${name}"? This unlinks it from the repo; the folder stays on disk.`)) return;
    await ipc.removeWorktree(id, name).catch((e) => setError(`Couldn't remove worktree: ${String(e)}`));
    setWorktrees(await ipc.listWorktrees(id).catch(() => worktrees));
  };

  // Create a worktree (+ branch) named `newWt`, placed in a sibling directory.
  const addWt = async () => {
    const name = newWt.trim();
    if (!id || !name || busy) return;
    const dest = `${id}-${name.replace(/[^A-Za-z0-9._-]/g, "-")}`;
    setBusy(true);
    setError(null);
    try {
      await ipc.addWorktree(id, name, dest);
      setNewWt("");
      setWorktrees(await ipc.listWorktrees(id));
    } catch (e) {
      setError(`Couldn't add worktree: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // Project tags for this repo (shared store; syncs with the sidebar facet).
  const tags = (id && tagMap[id]) || [];
  const addTag = () => {
    const t = newTag.trim();
    if (!id || !t) return;
    setRepoTags(id, [...tags, t]);
    setNewTag("");
  };
  const removeTag = (t: string) => {
    if (id) setRepoTags(id, tags.filter((x) => x !== t));
  };

  const genCommitMsg = async () => {
    if (!id || aiBusy) return;
    setAiBusy(true);
    setError(null);
    try {
      setCommitMsg(await ipc.generateCommitMessage(id));
    } catch (e) {
      setError(String(e));
    } finally {
      setAiBusy(false);
    }
  };

  const doCommit = async () => {
    if (!id || !commitMsg.trim() || aiBusy) return;
    setAiBusy(true);
    try {
      await ipc.commitStaged(id, commitMsg);
      setCommitMsg("");
      setDiff(await ipc.repoStagedDiff(id).catch(() => ""));
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setAiBusy(false);
    }
  };

  const genChangelog = async () => {
    if (!id || aiBusy) return;
    setAiBusy(true);
    setError(null);
    try {
      setChangelog(await ipc.generateChangelog(id, 20));
    } catch (e) {
      setError(String(e));
    } finally {
      setAiBusy(false);
    }
  };

  const prunable = branches.some((b) => !b.isHead && (b.merged || b.gone));

  return (
    <div className="fixed inset-0 z-30" role="dialog" aria-modal="true">
      <div className="orr-drawer-scrim absolute inset-0 bg-black/40" onClick={onClose} />
      <aside className="orr-drawer-panel absolute right-0 top-0 flex h-full w-full max-w-[560px] flex-col border-l border-border bg-card shadow-2xl">
        {/* Header */}
        <div className="flex items-start gap-3 border-b border-border/70 p-4">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              {repo.host && <HostIcon host={repo.host} className="size-4 opacity-70" />}
              <h2 className="truncate text-lg font-semibold tracking-tight">{repo.displayName}</h2>
            </div>
            <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
              {repo.slug ?? "no remote"} · {repo.path}
            </p>
            {repo.aiSummary && <p className="mt-2 text-sm text-muted-foreground">{repo.aiSummary}</p>}
          </div>
          <button type="button" className="orr-iconbtn" aria-label="Close" onClick={onClose}>
            <X className="size-4" />
          </button>
        </div>

        {/* Tabs — the PRs tab only applies to GitHub repos with a remote. */}
        <div className="flex gap-1 border-b border-border/70 px-3 py-2 text-sm">
          {(["overview", "changes", "pr", "notes", "readme"] as Tab[])
            .filter((t) => t !== "pr" || (repo.host === "github" && !!repo.slug))
            .map((t) => (
              <button
                key={t}
                type="button"
                onClick={() => setTab(t)}
                className={cn(
                  "rounded-md px-3 py-1",
                  tab === t ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                {TAB_LABEL[t]}
              </button>
            ))}
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          {tab === "overview" && (
            <div className="space-y-6">
              {/* Branches */}
              <section>
                <div className="mb-2 flex items-center justify-between">
                  <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Branches</h3>
                  {prunable && (
                    <button
                      type="button"
                      className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
                      onClick={doPrune}
                      disabled={busy}
                    >
                      <Scissors className="size-3.5" /> Prune merged/gone
                    </button>
                  )}
                </div>
                {error && <p className="mb-2 text-sm text-danger">{error}</p>}
                {branches.length === 0 ? (
                  <p className="text-sm text-muted-foreground">No branches.</p>
                ) : (
                  <VirtualList
                    items={branches}
                    getKey={(b) => b.name}
                    estimateSize={34}
                    gap={4}
                    className="max-h-80"
                    renderItem={(b) => (
                      <button
                        type="button"
                        disabled={b.isHead || busy}
                        onClick={() => doSwitch(b.name)}
                        className={cn(
                          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm",
                          b.isHead ? "bg-secondary/60" : "hover:bg-secondary/40",
                        )}
                      >
                        <GitBranch className="size-3.5 text-muted-foreground" />
                        <span className={cn("font-mono", b.isHead && "font-semibold")}>{b.name}</span>
                        {b.isHead && <Check className="size-3.5 text-ok" />}
                        {b.merged && !b.isHead && <span className="ml-auto text-xs text-muted-foreground">merged</span>}
                        {b.gone && <span className="ml-auto text-xs text-warn">gone</span>}
                      </button>
                    )}
                  />
                )}
              </section>

              {/* Projects / tags */}
              <section>
                <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Projects</h3>
                <div className="flex flex-wrap items-center gap-1.5">
                  {tags.map((t) => (
                    <span key={t} className="inline-flex items-center gap-1 rounded-md border border-border bg-secondary/40 px-2 py-0.5 text-xs">
                      <Tag className="size-3 text-muted-foreground" />
                      {t}
                      <button type="button" className="text-muted-foreground hover:text-danger" aria-label={`Remove ${t}`} onClick={() => removeTag(t)}>
                        <X className="size-3" />
                      </button>
                    </span>
                  ))}
                  <form
                    className="inline-flex items-center"
                    onSubmit={(e) => {
                      e.preventDefault();
                      addTag();
                    }}
                  >
                    <input
                      className="w-28 rounded-md border border-border bg-background/50 px-2 py-0.5 text-xs outline-none focus:border-primary/50"
                      value={newTag}
                      spellCheck={false}
                      placeholder="+ project"
                      onChange={(e) => setNewTag(e.target.value)}
                    />
                  </form>
                </div>
              </section>

              {/* Worktrees */}
              <section>
                <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Worktrees</h3>
                <div className="space-y-1">
                  {worktrees.map((w) => (
                    <div key={w.name} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm">
                      <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />
                      <span className="font-mono">{w.name}</span>
                      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{w.path}</span>
                      <button type="button" className="text-muted-foreground hover:text-foreground" title="Open in IDE" aria-label="Open in IDE" onClick={() => isTauri() && ipc.openInIde(w.path).catch(() => {})}>
                        <Code className="size-3.5" />
                      </button>
                      <button type="button" className="text-muted-foreground hover:text-foreground" title="Open agent here" aria-label="Open agent here" onClick={() => isTauri() && ipc.openAgent(w.path).catch(() => {})}>
                        <SquareTerminal className="size-3.5" />
                      </button>
                      <button type="button" className="text-muted-foreground hover:text-foreground" title="Open folder" aria-label="Open folder" onClick={() => isTauri() && ipc.openFolder(w.path).catch(() => {})}>
                        <FolderOpen className="size-3.5" />
                      </button>
                      <button type="button" className="text-muted-foreground hover:text-danger" title="Remove worktree" aria-label="Remove worktree" onClick={() => removeWt(w.name)}>
                        <Trash2 className="size-3.5" />
                      </button>
                    </div>
                  ))}
                  {worktrees.length === 0 && (
                    <p className="px-2 text-xs text-muted-foreground">No linked worktrees. Create one to work on a branch in parallel — or to drop an agent into an isolated tree.</p>
                  )}
                </div>
                <form
                  className="mt-2 flex items-center gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    addWt();
                  }}
                >
                  <input
                    className="min-w-0 flex-1 rounded-md border border-border bg-background/50 px-2 py-1.5 font-mono text-xs outline-none focus:border-primary/50"
                    value={newWt}
                    spellCheck={false}
                    placeholder="new-branch-name"
                    onChange={(e) => setNewWt(e.target.value)}
                    disabled={busy}
                  />
                  <button
                    type="submit"
                    className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1.5 text-xs text-muted-foreground hover:text-foreground disabled:opacity-40"
                    disabled={busy || !newWt.trim()}
                  >
                    <Plus className="size-3.5" /> Add worktree
                  </button>
                </form>
              </section>

              {/* Recent commits */}
              <section>
                <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Recent commits</h3>
                <div className="space-y-2">
                  {log.map((c) => (
                    <div key={c.id} className="flex gap-2 text-sm">
                      <code className="shrink-0 text-xs text-muted-foreground">{c.id}</code>
                      <span className="min-w-0 flex-1 truncate">{c.summary}</span>
                      <span className="shrink-0 text-xs text-muted-foreground">{timeAgo(c.timeUnix)}</span>
                    </div>
                  ))}
                  {log.length === 0 && <p className="text-sm text-muted-foreground">No commits.</p>}
                </div>
              </section>
            </div>
          )}

          {tab === "changes" && (
            <div className="space-y-3">
              {aiReady && (
                <div className="flex flex-wrap gap-2">
                  <button type="button" className="orr-cbtn" onClick={genCommitMsg} disabled={aiBusy}>
                    <Sparkles className="size-3.5" /> {aiBusy ? "Thinking…" : "Commit message"}
                  </button>
                  <button type="button" className="orr-cbtn" onClick={genChangelog} disabled={aiBusy}>
                    <Sparkles className="size-3.5" /> Changelog
                  </button>
                </div>
              )}

              {commitMsg && (
                <div className="space-y-2">
                  <textarea
                    className="w-full rounded-md border border-border bg-background/60 p-2 font-mono text-xs"
                    rows={4}
                    value={commitMsg}
                    onChange={(e) => setCommitMsg(e.target.value)}
                  />
                  <button type="button" className="orr-cbtn ide" onClick={doCommit} disabled={aiBusy}>
                    <Check className="size-3.5" /> Commit staged
                  </button>
                </div>
              )}

              {changelog && (
                <div className="orr-md rounded-md bg-background/60 p-3 text-sm">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{changelog}</ReactMarkdown>
                </div>
              )}

              {error && <p className="text-sm text-danger">{error}</p>}

              {diff ? (
                <pre className="overflow-x-auto rounded-md bg-background/60 p-3 font-mono text-xs leading-relaxed">
                  {diff}
                </pre>
              ) : (
                <p className="text-sm text-muted-foreground">Nothing staged — `git add` changes to stage them.</p>
              )}
            </div>
          )}

          {tab === "pr" && repo.slug && <PrPanel slug={repo.slug} />}

          {tab === "notes" && <NotesPanel id={repo.id} aiReady={aiReady} />}

          {tab === "readme" &&
            (readme ? (
              <div className="orr-md text-sm">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{readme}</ReactMarkdown>
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">No README.</p>
            ))}
        </div>

        {/* Footer actions */}
        <div className="flex gap-2 border-t border-border/70 p-3">
          <button type="button" className="orr-cbtn ide flex-1" onClick={() => openIde(repo)}>
            <BrandIcon brand={ideBrand} category="ide" />
            {ideName || "Open in IDE"}
          </button>
          <button type="button" className="orr-cbtn agent flex-1" onClick={() => openAgent(repo)}>
            <BrandIcon brand={agentBrand} category="agent" />
            {agentName || "Agent"}
          </button>
        </div>
      </aside>
    </div>
  );
}
