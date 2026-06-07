import { useEffect, useState } from "react";
import { FolderGit2, GitBranchPlus } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { ipc, isTauri } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { cn } from "@/lib/utils";

type Mode = "create" | "clone";

export function NewProjectDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { refresh } = useRepos();
  const [mode, setMode] = useState<Mode>("create");
  const [roots, setRoots] = useState<string[]>([]);
  const [root, setRoot] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Create fields
  const [name, setName] = useState("");
  const [remote, setRemote] = useState("");
  const [template, setTemplate] = useState("");
  const [firstCommit, setFirstCommit] = useState(true);
  // Clone fields
  const [url, setUrl] = useState("");

  // Load workspace roots when the dialog opens; reset transient state.
  useEffect(() => {
    if (!open) return;
    setError(null);
    setBusy(false);
    if (!isTauri()) {
      setRoots(["~/dev"]);
      setRoot("~/dev");
      return;
    }
    ipc
      .getConfig()
      .then((c) => {
        const rs = c.roots.length ? c.roots : ["~/dev"];
        setRoots(rs);
        setRoot((prev) => (rs.includes(prev) ? prev : rs[0]));
      })
      .catch(() => {
        setRoots(["~/dev"]);
        setRoot("~/dev");
      });
  }, [open]);

  const reset = () => {
    setName("");
    setRemote("");
    setTemplate("");
    setUrl("");
    setFirstCommit(true);
    setError(null);
  };

  const canSubmit = mode === "create" ? name.trim().length > 0 && !!root : url.trim().length > 0 && !!root;

  const submit = async () => {
    if (!canSubmit || busy) return;
    if (!isTauri()) {
      setError("Creating projects is only available in the desktop app.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (mode === "create") {
        await ipc.initRepo(
          root,
          name.trim(),
          template.trim() || undefined,
          remote.trim() || undefined,
          firstCommit ? "Initial commit" : undefined,
        );
      } else {
        await ipc.cloneRepo(url.trim(), root);
      }
      refresh();
      reset();
      onOpenChange(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New</DialogTitle>
          <DialogDescription>Start a project or clone any git URL into a workspace root.</DialogDescription>
        </DialogHeader>

        {/* Mode toggle */}
        <div className="flex gap-1 rounded-md border border-border p-1 text-sm">
          {(["create", "clone"] as Mode[]).map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => {
                setMode(m);
                setError(null);
              }}
              className={cn(
                "flex flex-1 items-center justify-center gap-1.5 rounded px-3 py-1.5",
                mode === m ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {m === "create" ? <GitBranchPlus className="size-3.5" /> : <FolderGit2 className="size-3.5" />}
              {m === "create" ? "New project" : "Clone URL"}
            </button>
          ))}
        </div>

        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          {/* Destination root (shared) */}
          <label className="block space-y-1.5">
            <span className="text-sm text-muted-foreground">Workspace root</span>
            <select
              className="w-full rounded-md border border-border bg-background/60 px-2 py-2 text-sm outline-none focus:border-primary/50"
              value={root}
              onChange={(e) => setRoot(e.target.value)}
            >
              {roots.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </label>

          {mode === "create" ? (
            <>
              <label className="block space-y-1.5">
                <span className="text-sm text-muted-foreground">Project name</span>
                <Input value={name} spellCheck={false} placeholder="my-new-thing" onChange={(e) => setName(e.target.value)} autoFocus />
              </label>
              <label className="block space-y-1.5">
                <span className="text-sm text-muted-foreground">Remote URL (optional)</span>
                <Input value={remote} spellCheck={false} placeholder="git@github.com:me/my-new-thing.git" onChange={(e) => setRemote(e.target.value)} />
              </label>
              <label className="block space-y-1.5">
                <span className="text-sm text-muted-foreground">Template directory (optional)</span>
                <Input value={template} spellCheck={false} placeholder="~/dev/templates/rust-cli" onChange={(e) => setTemplate(e.target.value)} />
              </label>
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" checked={firstCommit} onChange={(e) => setFirstCommit(e.target.checked)} />
                Make a first commit
              </label>
            </>
          ) : (
            <label className="block space-y-1.5">
              <span className="text-sm text-muted-foreground">Repository URL</span>
              <Input value={url} spellCheck={false} placeholder="https://github.com/owner/repo.git" onChange={(e) => setUrl(e.target.value)} autoFocus />
            </label>
          )}

          {error && <p className="text-sm text-danger">{error}</p>}

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit || busy}>
              {busy ? (mode === "create" ? "Creating…" : "Cloning…") : mode === "create" ? "Create" : "Clone"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
