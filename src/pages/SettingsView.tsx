import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, DownloadCloud, FolderGit2, LogOut, Plus, Sparkles, Terminal, Trash2, Zap } from "lucide-react";
import { ipc, isTauri, type AiStatus, type AiTest, type AppConfig, type DeviceStart } from "@/lib/ipc";
import { reduceMotionEnabled, setReduceMotion } from "@/lib/motion";
import { useRepos } from "@/lib/repos-context";
import { HostIcon } from "@/components/HostIcon";
import { ModelSelect } from "@/components/ModelSelect";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

const FALLBACK: AppConfig = {
  roots: ["~/dev"],
  scanDepth: 3,
  ignore: ["node_modules", ".cache", "vendor", "target", "dist", ".git"],
  ideCommand: "code {path}",
  agentCommand: "kitty --working-directory {path} -e claude",
  githubClientId: "",
  gitlabHosts: [],
  aiModel: "granite4.1:3b-q2_K",
  aiEnabled: true,
  embedModel: "nomic-embed-text:latest",
  ollamaHost: "http://localhost:11434",
};

interface PullState {
  model: string;
  status: string;
  percent: number;
}

export function SettingsView() {
  const { refresh } = useRepos();
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [authed, setAuthed] = useState(false);
  const [device, setDevice] = useState<DeviceStart | null>(null);
  const [loginError, setLoginError] = useState<string | null>(null);
  const [aiStatus, setAiStatus] = useState<AiStatus | null>(null);
  const [aiTesting, setAiTesting] = useState(false);
  const [aiTest, setAiTest] = useState<AiTest | null>(null);
  const [pulling, setPulling] = useState<PullState | null>(null);
  const [pullError, setPullError] = useState<string | null>(null);
  const [reduceMotion, setReduceMotionState] = useState(reduceMotionEnabled);

  useEffect(() => {
    if (!isTauri()) {
      setConfig(FALLBACK);
      return;
    }
    ipc.getConfig().then(setConfig).catch(() => setConfig(FALLBACK));
    ipc.githubAuthStatus().then(setAuthed).catch(() => {});
    ipc.aiStatus().then(setAiStatus).catch(() => {});
  }, []);

  // Device-flow polling lives in an effect so it's cancelled if the user
  // navigates away mid-login. Honors `slow_down` by widening the interval.
  useEffect(() => {
    if (!device) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    let intervalMs = (device.interval + 1) * 1000;
    const poll = async () => {
      try {
        const { status } = await ipc.githubLoginPoll(device.deviceCode);
        if (cancelled) return;
        if (status === "authorized") {
          setAuthed(true);
          setDevice(null);
          refresh();
        } else if (status === "authorization_pending" || status === "slow_down") {
          if (status === "slow_down") intervalMs += 5000;
          timer = setTimeout(poll, intervalMs);
        } else {
          setLoginError(status);
          setDevice(null);
        }
      } catch (e) {
        if (!cancelled) {
          setLoginError(String(e));
          setDevice(null);
        }
      }
    };
    timer = setTimeout(poll, intervalMs);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [device, refresh]);

  // Live pull progress from the backend (`ollama pull` streams NDJSON).
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    listen<PullState>("pull-progress", (e) => setPulling(e.payload))
      .then((fn) => (unlisten = fn))
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  if (!config) return <div className="orr-settings" />;

  const installedModels = aiStatus?.models ?? [];
  // Ollama reports names with tags (e.g. "…:latest"); treat a tagless value as
  // matching its :latest variant so the installed check stays accurate.
  const has = (m: string) => {
    const t = m.trim();
    return installedModels.includes(t) || (!t.includes(":") && installedModels.includes(`${t}:latest`));
  };

  const doPull = async (raw: string) => {
    const model = raw.trim();
    if (!isTauri() || !model || pulling) return;
    setPullError(null);
    setPulling({ model, status: "starting", percent: 0 });
    try {
      await ipc.setConfig(config); // ensure the pull uses the current endpoint
      await ipc.pullModel(model);
      setAiStatus(await ipc.aiStatus()); // refresh the installed list
    } catch (e) {
      setPullError(String(e));
    } finally {
      setPulling(null);
    }
  };

  // Installed ✓ / not-installed + a Pull button (live progress while pulling).
  const modelStatus = (raw: string) => {
    const model = raw.trim();
    if (!aiStatus?.reachable || !model) return null;
    if (has(model)) return <p className="text-xs text-ok">Installed ✓</p>;
    if (pulling?.model === model) {
      return (
        <p className="text-xs text-muted-foreground">
          Pulling… {pulling.status}
          {pulling.percent > 0 ? ` ${pulling.percent}%` : ""}
        </p>
      );
    }
    return (
      <div className="flex items-center gap-2 text-xs text-warn">
        <span>Not installed.</span>
        <button
          type="button"
          className="inline-flex items-center gap-1 text-primary hover:underline disabled:opacity-50"
          onClick={() => doPull(model)}
          disabled={!!pulling}
        >
          <DownloadCloud className="size-3.5" /> Pull {model}
        </button>
      </div>
    );
  };

  const patch = (p: Partial<AppConfig>) => {
    setConfig({ ...config, ...p });
    setSaved(false);
  };

  const save = async () => {
    try {
      if (isTauri()) await ipc.setConfig(config);
      setSaveError(null);
      setSaved(true);
      refresh();
    } catch (e) {
      setSaved(false);
      setSaveError(String(e));
      console.error("[orrery] save config:", e);
    }
  };

  // Persist the current AI settings, then probe status + run a live generate/
  // embed so "Test" reflects exactly what's typed (not the last-saved values).
  const testAi = async () => {
    setAiTesting(true);
    setAiTest(null);
    try {
      if (isTauri()) await ipc.setConfig(config);
      const [status, test] = await Promise.all([ipc.aiStatus(), ipc.aiTest()]);
      setAiStatus(status);
      setAiTest(test);
    } catch (e) {
      setAiTest({ chatOk: false, embedOk: false, ms: 0, error: String(e) });
    } finally {
      setAiTesting(false);
    }
  };

  const connectGithub = async () => {
    setLoginError(null);
    try {
      // Persist the client id first so the backend can use it, then start the
      // flow. The polling effect (keyed on `device`) takes it from here.
      if (isTauri()) await ipc.setConfig(config);
      setDevice(await ipc.githubLoginStart());
    } catch (e) {
      setLoginError(String(e));
    }
  };

  const signOutGithub = async () => {
    if (isTauri()) await ipc.githubSignOut().catch(() => {});
    setAuthed(false);
  };

  return (
    <div className="orr-settings">
      <header className="orr-settings-head">
        <h1>Settings</h1>
        <p>
          Stored at <code>~/.config/orrery/config.toml</code>
        </p>
      </header>

      <div className="orr-settings-body">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <FolderGit2 className="size-4 text-primary" /> Workspace roots
            </CardTitle>
            <CardDescription>Directories scanned for git repos.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {config.roots.map((root, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  value={root}
                  spellCheck={false}
                  onChange={(e) => {
                    const roots = [...config.roots];
                    roots[i] = e.target.value;
                    patch({ roots });
                  }}
                />
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label="Remove root"
                  onClick={() => patch({ roots: config.roots.filter((_, j) => j !== i) })}
                >
                  <Trash2 className="size-4" />
                </Button>
              </div>
            ))}
            <Button variant="outline" size="sm" onClick={() => patch({ roots: [...config.roots, "~/"] })}>
              <Plus className="size-4" /> Add root
            </Button>

            <div className="flex items-center gap-3 pt-2">
              <label className="text-sm text-muted-foreground" htmlFor="depth">
                Scan depth
              </label>
              <Input
                id="depth"
                type="number"
                min={1}
                max={8}
                className="w-20"
                value={config.scanDepth}
                onChange={(e) => patch({ scanDepth: Math.max(1, Number(e.target.value) || 1) })}
              />
            </div>

            <div className="space-y-1.5 pt-1">
              <label className="text-sm text-muted-foreground" htmlFor="ignore">
                Ignore (comma-separated)
              </label>
              <Input
                id="ignore"
                spellCheck={false}
                value={config.ignore.join(", ")}
                onChange={(e) =>
                  patch({ ignore: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })
                }
              />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Terminal className="size-4 text-primary" /> Launchers
            </CardTitle>
            <CardDescription>
              <code>{"{path}"}</code> is replaced with the repo path.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="space-y-1.5">
              <label className="text-sm text-muted-foreground" htmlFor="ide">
                Open in IDE
              </label>
              <Input id="ide" spellCheck={false} value={config.ideCommand} onChange={(e) => patch({ ideCommand: e.target.value })} />
            </div>
            <div className="space-y-1.5">
              <label className="text-sm text-muted-foreground" htmlFor="agent">
                Terminal agent
              </label>
              <Input id="agent" spellCheck={false} value={config.agentCommand} onChange={(e) => patch({ agentCommand: e.target.value })} />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <HostIcon host="github" className="size-4" /> GitHub
            </CardTitle>
            <CardDescription>
              Optional — connect for higher rate limits and private-repo enrichment. Public repos enrich
              without signing in (and an authenticated <code>gh</code> CLI is used automatically).
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {authed ? (
              <div className="flex items-center gap-3">
                <span className="text-sm text-ok">Connected.</span>
                <Button variant="outline" size="sm" onClick={signOutGithub}>
                  <LogOut className="size-4" /> Sign out
                </Button>
              </div>
            ) : (
              <>
                <div className="space-y-1.5">
                  <label className="text-sm text-muted-foreground" htmlFor="ghclient">
                    OAuth app client id (for device-flow login)
                  </label>
                  <Input
                    id="ghclient"
                    spellCheck={false}
                    placeholder="Iv1.xxxxxxxx"
                    value={config.githubClientId}
                    onChange={(e) => patch({ githubClientId: e.target.value })}
                  />
                </div>
                {device ? (
                  <p className="text-sm text-muted-foreground">
                    Open <code className="text-foreground">{device.verificationUri}</code> and enter code{" "}
                    <code className="text-foreground">{device.userCode}</code> — waiting…
                  </p>
                ) : (
                  <Button size="sm" onClick={connectGithub} disabled={!config.githubClientId}>
                    Connect GitHub
                  </Button>
                )}
                {loginError && <span className="text-sm text-danger">Login failed: {loginError}</span>}
              </>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Sparkles className="size-4 text-primary" /> AI &amp; semantic search
            </CardTitle>
            <CardDescription>
              Local-only via Ollama — powers repo summaries, commit messages, the daily briefing, and
              semantic search.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            {/* Connection status + live test */}
            <div className="flex items-center gap-2 rounded-md border border-border bg-background/40 px-3 py-2 text-sm">
              <span
                className={`size-2 shrink-0 rounded-full ${aiStatus?.reachable ? "bg-ok" : "bg-danger"}`}
              />
              <span className="min-w-0 flex-1 truncate">
                {!aiStatus ? (
                  "Checking…"
                ) : aiStatus.reachable ? (
                  <>
                    Connected to <code className="text-foreground">{aiStatus.endpoint}</code>
                  </>
                ) : (
                  aiStatus.error ?? "Not reachable"
                )}
              </span>
              <Button variant="outline" size="sm" onClick={testAi} disabled={aiTesting}>
                {aiTesting ? "Testing…" : "Test"}
              </Button>
            </div>
            {aiTest &&
              (aiTest.error ? (
                <p className="text-sm text-danger">Test failed: {aiTest.error}</p>
              ) : (
                <p className="text-sm text-ok">
                  Chat {aiTest.chatOk ? "✓" : "✗"} · Embeddings {aiTest.embedOk ? "✓" : "✗"} · {aiTest.ms} ms
                </p>
              ))}

            {/* Endpoint */}
            <div className="space-y-1.5">
              <label className="text-sm text-muted-foreground" htmlFor="ollama">
                Ollama endpoint
              </label>
              <Input
                id="ollama"
                spellCheck={false}
                placeholder="http://localhost:11434"
                value={config.ollamaHost}
                onChange={(e) => patch({ ollamaHost: e.target.value })}
              />
            </div>

            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={config.aiEnabled}
                onChange={(e) => patch({ aiEnabled: e.target.checked })}
              />
              Generate summaries, commit messages &amp; briefing
            </label>

            {/* Chat model */}
            <div className="space-y-1.5">
              <label className="text-sm text-muted-foreground" htmlFor="aimodel">
                Chat model
              </label>
              <ModelSelect
                id="aimodel"
                models={installedModels}
                value={config.aiModel}
                onChange={(aiModel) => patch({ aiModel })}
                disabled={!config.aiEnabled}
                placeholder="granite4.1:3b-q2_K"
              />
              {modelStatus(config.aiModel)}
            </div>

            {/* Embedding model */}
            <div className="space-y-1.5">
              <label className="text-sm text-muted-foreground" htmlFor="embedmodel">
                Embedding model (semantic search)
              </label>
              <ModelSelect
                id="embedmodel"
                models={installedModels}
                value={config.embedModel}
                onChange={(embedModel) => patch({ embedModel })}
                placeholder="nomic-embed-text:latest"
              />
              {modelStatus(config.embedModel)}
            </div>

            <p className="text-xs text-muted-foreground">
              Installed: {installedModels.join(", ") || "none — run `ollama pull <model>`"}
            </p>
            {pullError && <p className="text-xs text-danger">Pull failed: {pullError}</p>}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Zap className="size-4 text-primary" /> Motion
            </CardTitle>
            <CardDescription>
              Disable UI animations. Helps smoothness in the desktop webview, where some GPUs
              (notably NVIDIA on Linux) can’t accelerate them. Applies instantly — no rescan.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={reduceMotion}
                onChange={(e) => {
                  setReduceMotion(e.target.checked);
                  setReduceMotionState(e.target.checked);
                }}
              />
              Reduce motion
            </label>
          </CardContent>
        </Card>

        <div className="flex items-center gap-3">
          <Button onClick={save}>
            <Check className="size-4" /> Save & rescan
          </Button>
          {saved && <span className="text-sm text-ok">Saved.</span>}
          {saveError && <span className="text-sm text-danger">Couldn’t save: {saveError}</span>}
        </div>
      </div>
    </div>
  );
}
