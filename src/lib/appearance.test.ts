import { describe, it, expect, beforeEach } from "vitest";
import { applyAppearance, type Appearance } from "@/lib/appearance";

const base: Appearance = {
  colorScheme: null,
  accent: null,
  windowBg: null,
  windowFg: null,
  baseBg: null,
};

beforeEach(() => {
  document.documentElement.className = "";
  document.documentElement.removeAttribute("style");
});

describe("applyAppearance — theme", () => {
  it("adds .dark for a dark scheme", () => {
    applyAppearance({ ...base, colorScheme: "dark" });
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("removes .dark for a light scheme", () => {
    document.documentElement.classList.add("dark");
    applyAppearance({ ...base, colorScheme: "light" });
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("falls back to prefers-color-scheme when no preference", () => {
    // jsdom has no matchMedia — stub it to report dark.
    // @ts-expect-error test stub
    window.matchMedia = (query: string) => ({
      matches: true,
      media: query,
      addEventListener() {},
      removeEventListener() {},
    });
    applyAppearance(base);
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});

describe("applyAppearance — accent", () => {
  it("maps the accent to --primary and picks a light fg on a mid/dark accent", () => {
    applyAppearance({ ...base, colorScheme: "dark", accent: { r: 61, g: 174, b: 233 } });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--primary")).toBe("rgb(61 174 233)");
    expect(root.style.getPropertyValue("--primary-foreground")).toContain("0.98");
  });

  it("picks a dark fg on a very bright accent", () => {
    applyAppearance({ ...base, colorScheme: "light", accent: { r: 250, g: 250, b: 250 } });
    expect(document.documentElement.style.getPropertyValue("--primary-foreground")).toContain("0.16");
  });

  it("clears accent overrides when none is provided", () => {
    document.documentElement.style.setProperty("--primary", "rgb(1 2 3)");
    applyAppearance({ ...base, colorScheme: "dark" });
    expect(document.documentElement.style.getPropertyValue("--primary")).toBe("");
  });
});

describe("applyAppearance — borrowed surfaces", () => {
  it("anchors --background/--foreground on the desktop window colours", () => {
    applyAppearance({
      ...base,
      colorScheme: "dark",
      windowBg: { r: 49, g: 49, b: 58 },
      windowFg: { r: 234, g: 234, b: 234 },
      baseBg: { r: 49, g: 49, b: 58 },
    });
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--background")).toBe("rgb(49 49 58)");
    expect(root.style.getPropertyValue("--foreground")).toBe("rgb(234 234 234)");
  });
});
