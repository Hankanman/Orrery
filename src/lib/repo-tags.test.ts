import { describe, expect, it } from "vitest";
import { normalizeTags, parseTags, tagCounts } from "./repo-tags";

describe("parseTags", () => {
  it("returns {} for null / invalid / array", () => {
    expect(parseTags(null)).toEqual({});
    expect(parseTags("nope")).toEqual({});
    expect(parseTags("[1,2]")).toEqual({});
  });
  it("keeps string-array entries and drops junk", () => {
    const raw = JSON.stringify({ "/a": ["client-x", "wip"], "/b": [1, "", "ok"], "/c": "no", "/d": [] });
    expect(parseTags(raw)).toEqual({ "/a": ["client-x", "wip"], "/b": ["ok"] });
  });
});

describe("normalizeTags", () => {
  it("trims, drops empties, dedupes", () => {
    expect(normalizeTags([" a ", "a", "", "  ", "b"])).toEqual(["a", "b"]);
  });
});

describe("tagCounts", () => {
  it("counts across repos, sorted by count then name", () => {
    const map = { "/a": ["x", "y"], "/b": ["x"], "/c": ["x", "z"] };
    expect(tagCounts(map)).toEqual([
      ["x", 3],
      ["y", 1],
      ["z", 1],
    ]);
  });
});
