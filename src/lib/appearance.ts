// Mirrors the Rust `Appearance` struct (see src-tauri/src/appearance.rs).
export interface Rgb {
  r: number;
  g: number;
  b: number;
}

export interface Appearance {
  /** "dark" | "light", or null for "no preference". */
  colorScheme: "dark" | "light" | null;
  accent: Rgb | null;
  /** Desktop window background (chrome). */
  windowBg: Rgb | null;
  /** Desktop window text colour. */
  windowFg: Rgb | null;
  /** Desktop view/content background. */
  baseBg: Rgb | null;
}

const css = (c: Rgb) => `rgb(${c.r} ${c.g} ${c.b})`;

const mix = (a: Rgb, b: Rgb, t: number): Rgb => ({
  r: Math.round(a.r + (b.r - a.r) * t),
  g: Math.round(a.g + (b.g - a.g) * t),
  b: Math.round(a.b + (b.b - a.b) * t),
});

function srgbToLinear(c: number): number {
  const s = c / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** WCAG relative luminance (0–1) of an 8-bit sRGB colour. */
function relativeLuminance({ r, g, b }: Rgb): number {
  return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
}

// Accent-driven variables (set from the system accent; cleared otherwise).
const ACCENT_VARS = ["--primary", "--ring", "--sidebar-primary", "--sidebar-ring"] as const;
const ACCENT_FG_VARS = ["--primary-foreground", "--sidebar-primary-foreground"] as const;

// Surface variables derived from the borrowed desktop colours. Cleared when no
// system colours are available so the branded stylesheet palette takes over.
const SURFACE_VARS = [
  "--background",
  "--card",
  "--popover",
  "--secondary",
  "--muted",
  "--accent",
  "--border",
  "--input",
  "--foreground",
  "--card-foreground",
  "--popover-foreground",
  "--secondary-foreground",
  "--accent-foreground",
  "--muted-foreground",
  "--sidebar",
  "--sidebar-foreground",
  "--sidebar-accent",
  "--sidebar-accent-foreground",
  "--sidebar-border",
] as const;

function resolveDark(scheme: Appearance["colorScheme"]): boolean {
  if (scheme === "dark") return true;
  if (scheme === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/**
 * Hybrid theming: anchor surfaces/text on the desktop's window colours (so we
 * harmonise with the native theme) while keeping Orrery's layered structure by
 * deriving elevations as small mixes toward the foreground. Mixing toward `fg`
 * lightens in dark mode and darkens in light mode — correct in both.
 */
function applySurfaces(root: HTMLElement, win: Rgb, fg: Rgb, view: Rgb): void {
  const set = (k: string, c: Rgb) => root.style.setProperty(k, css(c));
  const elevate = (t: number) => mix(win, fg, t);

  set("--background", win);
  set("--card", mix(view, fg, 0.04));
  set("--popover", mix(view, fg, 0.06));
  set("--secondary", elevate(0.09));
  set("--muted", elevate(0.06));
  set("--accent", elevate(0.12)); // neutral hover surface (shadcn uses --accent)
  set("--border", elevate(0.16));
  set("--input", elevate(0.16));

  set("--foreground", fg);
  set("--card-foreground", fg);
  set("--popover-foreground", fg);
  set("--secondary-foreground", fg);
  set("--accent-foreground", fg);
  set("--muted-foreground", elevate(0.55));

  set("--sidebar", win);
  set("--sidebar-foreground", fg);
  set("--sidebar-accent", elevate(0.09));
  set("--sidebar-accent-foreground", fg);
  set("--sidebar-border", elevate(0.16));
}

/** Apply the desktop's theme + accent + borrowed surfaces. Idempotent. */
export function applyAppearance(appearance: Appearance): void {
  const root = document.documentElement;

  const dark = resolveDark(appearance.colorScheme);
  root.classList.toggle("dark", dark);
  // Keep native form controls / scrollbars in step.
  root.style.colorScheme = dark ? "dark" : "light";

  if (appearance.windowBg && appearance.windowFg) {
    applySurfaces(root, appearance.windowBg, appearance.windowFg, appearance.baseBg ?? appearance.windowBg);
  } else {
    for (const v of SURFACE_VARS) root.style.removeProperty(v);
  }

  if (appearance.accent) {
    const accentCss = css(appearance.accent);
    for (const v of ACCENT_VARS) root.style.setProperty(v, accentCss);
    const fg = relativeLuminance(appearance.accent) > 0.45 ? "oklch(0.16 0.02 256)" : "oklch(0.98 0.01 250)";
    for (const v of ACCENT_FG_VARS) root.style.setProperty(v, fg);
  } else {
    for (const v of [...ACCENT_VARS, ...ACCENT_FG_VARS]) root.style.removeProperty(v);
  }
}
