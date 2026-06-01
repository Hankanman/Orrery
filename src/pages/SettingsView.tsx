import { useEffect, useState } from "react";
import { Check, FolderGit2, Plus, Terminal, Trash2 } from "lucide-react";
import { ipc, isTauri, type AppConfig } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";

const FALLBACK: AppConfig = {
  roots: ["~/dev"],
  scanDepth: 3,
  ignore: ["node_modules", ".cache", "vendor", "target", "dist", ".git"],
  ideCommand: "code {path}",
  agentCommand: "kitty --working-directory {path} -e claude",
};

export function SettingsView() {
  const { refresh } = useRepos();
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!isTauri()) {
      setConfig(FALLBACK);
      return;
    }
    ipc.getConfig().then(setConfig).catch(() => setConfig(FALLBACK));
  }, []);

  if (!config) return <div className="orr-settings" />;

  const patch = (p: Partial<AppConfig>) => {
    setConfig({ ...config, ...p });
    setSaved(false);
  };

  const save = async () => {
    if (isTauri()) await ipc.setConfig(config).catch((e) => console.error("[orrery] save config:", e));
    setSaved(true);
    refresh();
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

        <div className="flex items-center gap-3">
          <Button onClick={save}>
            <Check className="size-4" /> Save & rescan
          </Button>
          {saved && <span className="text-sm text-ok">Saved.</span>}
        </div>
      </div>
    </div>
  );
}
