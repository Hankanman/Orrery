import { useEffect, useMemo, useState } from "react";
import { Code, FolderOpen, RotateCw, SquareTerminal, X } from "lucide-react";
import { ipc, isTauri, type AgentSession } from "@/lib/ipc";
import { MOCK_AGENT_SESSIONS } from "@/lib/mock-activity";
import { detectAgent } from "@/lib/launchers";
import { useRepos } from "@/lib/repos-context";
import { timeAgo } from "@/lib/format";
import { Spinner } from "@/components/Spinner";

const POLL_MS = 5000;

export function AgentsView() {
  const { repos, openAgent, openFolder, openIde } = useRepos();
  const [sessions, setSessions] = useState<AgentSession[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = () => {
    if (!isTauri()) {
      setSessions(MOCK_AGENT_SESSIONS);
      return Promise.resolve();
    }
    return ipc.listAgentSessions().then(setSessions).catch(() => setSessions([]));
  };

  useEffect(() => {
    load();
    if (!isTauri()) return;
    const t = setInterval(load, POLL_MS);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const repoById = useMemo(() => new Map(repos.map((r) => [r.id, r])), [repos]);

  const terminate = async (id: string) => {
    if (!isTauri() || busy) return;
    setBusy(id);
    try {
      await ipc.killAgent(id);
      setSessions((s) => (s ?? []).filter((x) => x.id !== id));
    } catch {
      /* surfaced by the next poll */
    } finally {
      setBusy(null);
      load();
    }
  };

  const reopen = (id: string) => {
    const repo = repoById.get(id);
    if (repo) openAgent(repo);
    // Give the new terminal a beat to register, then refresh.
    setTimeout(load, 800);
  };

  return (
    <div className="orr-inbox">
      <header className="orr-settings-head">
        <h1>Agent Sessions</h1>
        <p>Terminal-agent sessions you've launched. Terminate or re-open them here.</p>
      </header>

      <div className="orr-settings-body">
        {sessions && sessions.length > 0 && (
          <div className="orr-inbox-list">
            {sessions.map((s) => {
              const repo = repoById.get(s.id);
              const name = repo?.displayName ?? s.id.split("/").pop() ?? s.id;
              const agent = detectAgent(s.command);
              return (
                <div key={s.id} className="orr-inbox-row" style={{ cursor: "default" }}>
                  <SquareTerminal className="size-4 shrink-0 animate-pulse text-primary" aria-hidden />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-medium">{name}</span>
                      {agent && <span className="orr-tag">{agent.name}</span>}
                    </div>
                    <span className="block truncate font-mono text-xs text-muted-foreground">{s.id}</span>
                  </div>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    started {timeAgo(s.startedAt)} · pid {s.pid}
                  </span>
                  <div className="flex shrink-0 items-center gap-1">
                    {repo && (
                      <>
                        <button
                          type="button"
                          className="orr-iconbtn"
                          title="Re-open agent"
                          aria-label="Re-open agent"
                          onClick={() => reopen(s.id)}
                        >
                          <RotateCw className="size-3.5" />
                        </button>
                        <button
                          type="button"
                          className="orr-iconbtn"
                          title="Open in IDE"
                          aria-label="Open in IDE"
                          onClick={() => openIde(repo)}
                        >
                          <Code className="size-3.5" />
                        </button>
                        <button
                          type="button"
                          className="orr-iconbtn"
                          title="Open folder"
                          aria-label="Open folder"
                          onClick={() => openFolder(repo)}
                        >
                          <FolderOpen className="size-3.5" />
                        </button>
                      </>
                    )}
                    <button
                      type="button"
                      className="orr-iconbtn hover:text-danger"
                      title="Terminate session"
                      aria-label="Terminate session"
                      disabled={busy === s.id}
                      onClick={() => terminate(s.id)}
                    >
                      <X className="size-3.5" />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}

        {sessions && sessions.length === 0 && (
          <div className="orr-empty">
            <SquareTerminal className="size-8 opacity-60" />
            <p className="t">No active sessions</p>
            <p className="s">Launch a coding agent from a repo card or the command palette and it'll show up here.</p>
          </div>
        )}

        {sessions === null && (
          <div className="orr-empty">
            <Spinner size={32} />
            <p className="s">Loading sessions…</p>
          </div>
        )}
      </div>
    </div>
  );
}
