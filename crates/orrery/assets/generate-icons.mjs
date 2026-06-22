// Generate the SVG icon assets the native app embeds, from the devDependency
// icon packages (lucide-react, simple-icons). Mirrors the repo convention:
// generated icon data is committed; the source packages stay devDeps and are
// never imported at runtime. Re-run from the repo root:
//   node crates/orrery/assets/generate-icons.mjs
//
// lucide icons are stroke-based; we emit stroke="#000" so usvg rasterizes the
// strokes into an alpha mask that GPUI's svg() element tints via text_color.
import { writeFileSync, mkdirSync, copyFileSync, existsSync } from "fs";
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
  // drawer (header/tabs/overview/pr/notes)
  "x", "check", "folder-tree", "history",
  "git-pull-request", "git-merge", "circle-check", "eye",
  // command palette
  "box", "file-search",
  // settings (status rows)
  "circle-alert",
];

// Language marks: multicolor devicon "-original" SVGs, rendered in full color
// by GPUI's img() element. Keyed by stem → devicon directory. Stems must mirror
// `devicon_stem()` in theme.rs.
const DEVICON = {
  rust: "rust", typescript: "typescript", javascript: "javascript", python: "python",
  go: "go", ruby: "ruby", java: "java", c: "c", cpp: "cplusplus", csharp: "csharp",
  html: "html5", css: "css3", shell: "bash", vue: "vuejs", svelte: "svelte",
  kotlin: "kotlin", swift: "swift", php: "php", scala: "scala", elixir: "elixir",
  haskell: "haskell", lua: "lua", dart: "dart", zig: "zig", nix: "nixos", markdown: "markdown",
};
const DEVICON_DIR = join(ROOT, "node_modules/devicon/icons");

mkdirSync(join(OUT, "lucide"), { recursive: true });
mkdirSync(join(OUT, "brand"), { recursive: true });
mkdirSync(join(OUT, "devicon"), { recursive: true });

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

// Language marks (multicolor devicon originals).
let devCount = 0;
for (const [stem, dir] of Object.entries(DEVICON)) {
  const src = join(DEVICON_DIR, dir, `${dir}-original.svg`);
  if (existsSync(src)) {
    copyFileSync(src, join(OUT, "devicon", `${stem}.svg`));
    devCount++;
  } else {
    console.warn(`  devicon missing: ${dir}-original.svg (lang ${stem})`);
  }
}

console.log(`generated ${LUCIDE_ICONS.length} lucide + 2 brand + ${devCount} devicon SVGs into ${OUT}`);
