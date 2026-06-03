import { describe, expect, it } from "vitest";
import { parseViews } from "./saved-views";

describe("parseViews", () => {
  it("returns [] for null / invalid / non-array", () => {
    expect(parseViews(null)).toEqual([]);
    expect(parseViews("not json")).toEqual([]);
    expect(parseViews('{"not":"array"}')).toEqual([]);
  });
  it("keeps well-formed entries and drops junk", () => {
    const raw = JSON.stringify([
      { id: "1", name: "Dirty", root: "all", lang: null, chips: ["dirty"], visibility: "all", attention: false, sort: "activity" },
      { id: "2", name: "bad-no-chips" }, // missing chips array → dropped
      { nope: true },
    ]);
    const views = parseViews(raw);
    expect(views).toHaveLength(1);
    expect(views[0].name).toBe("Dirty");
    expect(views[0].chips).toEqual(["dirty"]);
  });
});
