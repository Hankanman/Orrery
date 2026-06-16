// Generate the SVG icon assets the native app embeds, from the devDependency
// icon packages (lucide-react, simple-icons). Mirrors the repo convention:
// generated icon data is committed; the source packages stay devDeps and are
// never imported at runtime. Re-run from the repo root:
//   node crates/orrery-app/assets/generate-icons.mjs
//
// lucide icons are stroke-based; we emit stroke="#000" so usvg rasterizes the
// strokes into an alpha mask that GPUI's svg() element tints via text_color.
import { writeFileSync, mkdirSync, copyFileSync } from "fs";
import { join } from "path";

const ROOT = join(import.meta.dirname, "..", "..", ".."); // repo root
const LUCIDE = join(ROOT, "node_modules/lucide-react/dist/esm/icons");
const SIMPLE = join(ROOT, "node_modules/simple-icons/icons");
const OUT = join(import.meta.dirname, "icons");

const LUCIDE_ICONS = [
  // nav
  "layout-grid", "inbox", "rss", "compass", "square-terminal", "wrench", "scissors", "settings",
  // header
  "orbit", "search", "plus", "refresh-cw", "folder",
  // card status + meta + actions
  "git-branch", "arrow-up", "arrow-down", "circle-dot", "star", "clock", "tag", "lock",
  "folder-open", "external-link", "sparkles",
  // sidebar footer
  "hard-drive",
];

mkdirSync(join(OUT, "lucide"), { recursive: true });
mkdirSync(join(OUT, "brand"), { recursive: true });

for (const name of LUCIDE_ICONS) {
  const mod = await import(join(LUCIDE, `${name}.mjs`));
  const body = mod.__iconNode
    .map(([tag, attrs]) => {
      const a = Object.entries(attrs)
        .filter(([k]) => k !== "key")
        .map(([k, v]) => `${k}="${v}"`)
        .join(" ");
      return `<${tag} ${a}/>`;
    })
    .join("");
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" ` +
    `stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
  writeFileSync(join(OUT, "lucide", `${name}.svg`), svg);
}

// Host brand marks (monochrome single-path → tint fine).
for (const b of ["github", "gitlab"]) {
  copyFileSync(join(SIMPLE, `${b}.svg`), join(OUT, "brand", `${b}.svg`));
}

console.log(`generated ${LUCIDE_ICONS.length} lucide + 2 brand SVGs into ${OUT}`);
