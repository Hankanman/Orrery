// Mirrors the Rust `Appearance` struct (see src-tauri/src/appearance.rs).
export interface Accent {
  r: number;
  g: number;
  b: number;
}

export interface Appearance {
  /** "dark" | "light", or null for "no preference". */
  colorScheme: "dark" | "light" | null;
  accent: Accent | null;
}

// CSS variables driven by the system accent. Overriding these on :root
// (document.documentElement) wins over the stylesheet defaults.
const ACCENT_VARS = [
  "--primary",
  "--ring",
  "--sidebar-primary",
  "--sidebar-ring",
] as const;
const ACCENT_FG_VARS = ["--primary-foreground", "--sidebar-primary-foreground"] as const;

function srgbToLinear(c: number): number {
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

/** WCAG relative luminance of an sRGB [0,1] colour. */
function relativeLuminance({ r, g, b }: Accent): number {
  return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
}

function resolveDark(scheme: Appearance["colorScheme"]): boolean {
  if (scheme === "dark") return true;
  if (scheme === "light") return false;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** Apply the desktop's theme + accent to the document. Idempotent. */
export function applyAppearance(appearance: Appearance): void {
  const root = document.documentElement;

  root.classList.toggle("dark", resolveDark(appearance.colorScheme));

  if (appearance.accent) {
    const { r, g, b } = appearance.accent;
    const css = `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
    for (const v of ACCENT_VARS) root.style.setProperty(v, css);

    // Pick a foreground that stays legible on the accent.
    const fg =
      relativeLuminance(appearance.accent) > 0.45
        ? "oklch(0.18 0.02 256)"
        : "oklch(0.98 0.01 250)";
    for (const v of ACCENT_FG_VARS) root.style.setProperty(v, fg);
  } else {
    // No system accent → fall back to the stylesheet defaults.
    for (const v of [...ACCENT_VARS, ...ACCENT_FG_VARS]) root.style.removeProperty(v);
  }
}
