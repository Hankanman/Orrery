import { MOCK_REPOS } from "@/lib/mock-repos";
import { RepoCard } from "@/components/RepoCard";
import type { Repo } from "@/types";

export function GridView() {
  const repos = MOCK_REPOS;

  // Wiring to the Rust launcher lands with the "Launcher" issue. For now,
  // surface intent in the console so the buttons are demonstrably live.
  const openIde = (r: Repo) => console.log("[orrery] open in IDE:", r.path);
  const openAgent = (r: Repo) => console.log("[orrery] start agent:", r.path);

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6">
      <div className="mb-4 flex items-baseline justify-between">
        <h1 className="text-lg font-semibold tracking-tight">Workspace</h1>
        <p className="text-sm text-muted-foreground">
          {repos.length} repos{" "}
          <span className="text-xs">(mock data — scanner lands in Phase 1)</span>
        </p>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        {repos.map((repo) => (
          <RepoCard key={repo.id} repo={repo} onOpenIde={openIde} onOpenAgent={openAgent} />
        ))}
      </div>
    </div>
  );
}
