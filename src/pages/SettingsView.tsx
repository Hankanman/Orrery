import { FolderGit2, Terminal } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

// Placeholder settings surface. Real config (root dirs, scan depth, ignore
// globs, launcher templates) is wired in the "Config module" + "Settings
// dialog" issues, backed by ~/.config/orrery/config.toml.
export function SettingsView() {
  return (
    <div className="mx-auto max-w-3xl px-4 py-6">
      <h1 className="mb-1 text-lg font-semibold tracking-tight">Settings</h1>
      <p className="mb-6 text-sm text-muted-foreground">
        Configuration is stored at <code className="font-mono">~/.config/orrery/config.toml</code>.
      </p>

      <div className="space-y-4">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <FolderGit2 className="size-4 text-primary" />
              Workspace roots
            </CardTitle>
            <CardDescription>
              Directories Orrery scans for git repos. Configurable scan depth and ignore globs.
            </CardDescription>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            Coming in the “Config module” + “Settings dialog” issues.
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Terminal className="size-4 text-primary" />
              Launchers
            </CardTitle>
            <CardDescription>
              {"{path}"}-templated commands for your IDE and terminal coding agent.
            </CardDescription>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            Coming in the “Launcher” issue.
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
