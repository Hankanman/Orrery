import { describe, it, expect } from "vitest";
import { formatStars, languageColor, repoStatus, timeAgo } from "@/lib/format";
import type { Repo } from "@/types";

const repo = (over: Partial<Repo> = {}): Repo => ({
  id: "/x",
  displayName: "x",
  slug: null,
  path: "~/x",
  description: null,
  language: null,
  git: { branch: "main", ahead: 0, behind: 0, dirty: 0 },
  lastCommitUnix: 0,
  activity: "active",
  root: "~",
  host: null,
  stars: 0,
  favorite: false,
  aiSummary: null,
  ...over,
});

describe("timeAgo", () => {
  const now = 1_000_000_000;
  it("formats seconds", () => expect(timeAgo(now - 30, now)).toBe("30s ago"));
  it("formats hours", () => expect(timeAgo(now - 3 * 3600, now)).toBe("3h ago"));
  it("formats days", () => expect(timeAgo(now - 2 * 86400, now)).toBe("2d ago"));
  it("clamps future timestamps to 0s", () => expect(timeAgo(now + 100, now)).toBe("0s ago"));
});

describe("repoStatus priority", () => {
  it("dirty takes precedence over behind", () =>
    expect(repoStatus(repo({ git: { branch: "m", ahead: 0, behind: 2, dirty: 3 } }))).toBe("dirty"));
  it("behind when clean tree but behind upstream", () =>
    expect(repoStatus(repo({ git: { branch: "m", ahead: 0, behind: 2, dirty: 0 } }))).toBe("behind"));
  it("stale when clean and inactive", () => expect(repoStatus(repo({ activity: "stale" }))).toBe("stale"));
  it("clean otherwise", () => expect(repoStatus(repo())).toBe("clean"));
});

describe("formatStars", () => {
  it("leaves small counts", () => expect(formatStars(842)).toBe("842"));
  it("abbreviates thousands", () => expect(formatStars(1284)).toBe("1.3k"));
});

describe("languageColor", () => {
  it("returns a known colour", () => expect(languageColor("Rust")).toBe("#dea584"));
  it("falls back for unknown/null", () => expect(languageColor(null)).toBe("#8b949e"));
});
