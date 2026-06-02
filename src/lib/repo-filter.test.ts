import { describe, expect, it } from "vitest";
import { isPublic, matchesChip, matchesVisibility, needsAttention } from "./repo-filter";
import type { Repo } from "@/types";

const NOW = Math.floor(Date.now() / 1000);

function repo(over: Partial<Repo> = {}): Repo {
  return {
    id: "/x",
    displayName: "x",
    slug: "o/x",
    path: "~/dev/x",
    description: null,
    language: "Rust",
    git: { branch: "main", ahead: 0, behind: 0, dirty: 0 },
    lastCommitUnix: NOW,
    activity: "active",
    root: "~/dev",
    host: "github",
    stars: 0,
    favorite: false,
    aiSummary: null,
    ...over,
  };
}

describe("isPublic", () => {
  it("public when it has a remote and isn't private", () => {
    expect(isPublic(repo({ host: "github", private: false }))).toBe(true);
  });
  it("not public when private", () => {
    expect(isPublic(repo({ host: "github", private: true }))).toBe(false);
  });
  it("not public when there's no remote (local-only)", () => {
    expect(isPublic(repo({ host: null, slug: null }))).toBe(false);
  });
  it("treats missing private flag as public (pre-enrichment default)", () => {
    expect(isPublic(repo({ host: "gitlab", private: undefined }))).toBe(true);
  });
});

describe("matchesVisibility", () => {
  const pub = repo({ host: "github", private: false });
  const priv = repo({ host: "gitlab", private: true });
  const local = repo({ host: null, slug: null });

  it("all matches everything", () => {
    for (const r of [pub, priv, local]) expect(matchesVisibility(r, "all")).toBe(true);
  });
  it("public matches only public remotes", () => {
    expect(matchesVisibility(pub, "public")).toBe(true);
    expect(matchesVisibility(priv, "public")).toBe(false);
    expect(matchesVisibility(local, "public")).toBe(false);
  });
  it("private matches private remotes and local-only repos", () => {
    expect(matchesVisibility(priv, "private")).toBe(true);
    expect(matchesVisibility(local, "private")).toBe(true);
    expect(matchesVisibility(pub, "private")).toBe(false);
  });
  it("public/private partition every repo (all = public + private)", () => {
    for (const r of [pub, priv, local]) {
      expect(matchesVisibility(r, "public") !== matchesVisibility(r, "private")).toBe(true);
    }
  });
});

describe("matchesChip", () => {
  it("dirty / ahead / starred", () => {
    expect(matchesChip(repo({ git: { branch: "m", ahead: 0, behind: 0, dirty: 3 } }), "dirty")).toBe(true);
    expect(matchesChip(repo({ git: { branch: "m", ahead: 2, behind: 0, dirty: 0 } }), "ahead")).toBe(true);
    expect(matchesChip(repo({ favorite: true }), "starred")).toBe(true);
    expect(matchesChip(repo({ favorite: false }), "starred")).toBe(false);
  });
});

describe("needsAttention", () => {
  it("true for dirty / ahead / behind", () => {
    expect(needsAttention(repo({ git: { branch: "m", ahead: 0, behind: 2, dirty: 0 } }))).toBe(true);
  });
  it("false for a clean, recently-active repo", () => {
    expect(needsAttention(repo())).toBe(false);
  });
});
