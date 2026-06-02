// Pure predicates behind Mission Control's filters. Kept out of the view so they
// can be unit-tested (visibility in particular has been bug-prone).

import type { Repo } from "@/types";
import { repoStatus } from "@/lib/format";

export type SortKey = "activity" | "name" | "stars";
export type Chip = "dirty" | "ahead" | "starred" | "stale";
export type Visibility = "all" | "public" | "private";

export function matchesChip(repo: Repo, chip: Chip): boolean {
  switch (chip) {
    case "dirty":
      return repo.git.dirty > 0;
    case "ahead":
      return repo.git.ahead > 0;
    case "starred":
      return repo.favorite;
    case "stale":
      return repoStatus(repo) === "stale";
  }
}

/** A repo needs attention if it has uncommitted work, unpushed/behind commits, or is stale. */
export function needsAttention(repo: Repo): boolean {
  return repo.git.dirty > 0 || repo.git.ahead > 0 || repo.git.behind > 0 || repoStatus(repo) === "stale";
}

/** Public only if it has a remote that isn't private; everything else (private
 *  remotes and local-only repos, which aren't published) counts as private. */
export function isPublic(repo: Repo): boolean {
  return repo.host != null && !repo.private;
}

export function matchesVisibility(repo: Repo, visibility: Visibility): boolean {
  if (visibility === "public") return isPublic(repo);
  if (visibility === "private") return !isPublic(repo);
  return true;
}
