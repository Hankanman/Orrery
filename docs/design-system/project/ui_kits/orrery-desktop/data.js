/* Orrery UI kit — mock repo data. Modeled on real repos from the source account
   so the grid feels authentic. Not live data — a fixture for the prototype. */
window.ORR_DATA = {
  roots: [
    { id: 'dev', path: '~/dev', count: 8 },
    { id: 'work', path: '~/work/trustmarque', count: 2 },
  ],
  repos: [
    {
      name: 'Orrery', slug: 'Hankanman/Orrery', path: '~/dev/orrery', root: 'dev',
      lang: 'Rust', langColor: 'var(--lang-rust)', host: 'github',
      desc: 'every repo in orbit — a Linux-native command center for all your git repos.',
      branch: 'main', ahead: 2, behind: 0, dirty: 3, status: 'dirty',
      stars: 128, issues: 4, release: 'design-phase', commitAgo: '4h ago', stale: false, fav: true,
      ai: 'Active Tauri + React project in early development. Phase 1 (local-first grid) is underway; recent work touches the scanner and card layout.',
      commits: [
        { msg: 'feat: dense grid layout + glass cards', meta: 'you · 4h ago' },
        { msg: 'wip: scanner depth + ignore globs', meta: 'you · 9h ago' },
        { msg: 'docs: lock visual direction in DESIGN.md', meta: 'you · 2d ago' },
      ],
    },
    {
      name: 'Meetily-Local', slug: 'Hankanman/Meetily-Local', path: '~/dev/meetily', root: 'dev',
      lang: 'Rust', langColor: 'var(--lang-rust)', host: 'github',
      desc: 'Privacy-first AI meeting assistant — 4x faster Parakeet/Whisper live transcription, fully local.',
      branch: 'main', ahead: 0, behind: 0, dirty: 0, status: 'clean',
      stars: 842, issues: 12, release: 'v0.4.1', commitAgo: '1d ago', stale: false, fav: true,
      ai: 'Mature local-AI transcription app. Stable on main; recent releases focused on diarization accuracy and Ollama summarization.',
      commits: [
        { msg: 'release: v0.4.1 — diarization fixes', meta: 'you · 1d ago' },
        { msg: 'perf: parakeet streaming buffer', meta: 'contributor · 3d ago' },
      ],
    },
    {
      name: 'MusicAssistantNative', slug: 'Hankanman/MusicAssistantNative', path: '~/dev/ma-native', root: 'dev',
      lang: 'C++', langColor: 'var(--lang-cpp)', host: 'github',
      desc: 'A native Linux desktop client for Music Assistant. Built with Qt 6 and KDE Frameworks 6.',
      branch: 'feat/queue-ui', ahead: 5, behind: 2, dirty: 7, status: 'dirty',
      stars: 56, issues: 3, release: null, commitAgo: '2h ago', stale: false, fav: false,
      ai: 'Heavy active development on a feature branch. The queue UI is mid-refactor — 7 uncommitted changes and divergence from main.',
      commits: [
        { msg: 'wip: drag-reorder queue rows', meta: 'you · 2h ago' },
        { msg: 'feat: speaker selection popover', meta: 'you · 6h ago' },
      ],
    },
    {
      name: 'nextdocs', slug: 'Hankanman/nextdocs', path: '~/dev/nextdocs', root: 'dev',
      lang: 'TypeScript', langColor: 'var(--lang-ts)', host: 'github',
      desc: 'Documentation site framework. Markdown-first, statically exported.',
      branch: 'main', ahead: 0, behind: 0, dirty: 0, status: 'clean',
      stars: 9, issues: 0, release: 'v1.2.0', commitAgo: '6d ago', stale: false, fav: false,
      ai: 'Clean, low-activity TypeScript project. Last release v1.2.0 a week ago; nothing pending.',
      commits: [
        { msg: 'release: v1.2.0', meta: 'you · 6d ago' },
        { msg: 'fix: nav anchor scroll offset', meta: 'you · 8d ago' },
      ],
    },
    {
      name: 'Changelog-Weaver', slug: 'Hankanman/Changelog-Weaver', path: '~/dev/changelog-weaver', root: 'dev',
      lang: 'Python', langColor: 'var(--lang-python)', host: 'github',
      desc: 'Use AI to generate release notes for your projects.',
      branch: 'main', ahead: 1, behind: 0, dirty: 0, status: 'clean',
      stars: 213, issues: 7, release: 'v2.0.0', commitAgo: '12d ago', stale: false, fav: false,
      ai: 'Popular Python utility. One unpushed commit; otherwise stable since the v2.0.0 release.',
      commits: [
        { msg: 'feat: GitLab provider support', meta: 'you · 12d ago' },
      ],
    },
    {
      name: 'tree-sitter-zmodel', slug: 'Hankanman/tree-sitter-zmodel', path: '~/dev/ts-zmodel', root: 'dev',
      lang: 'JavaScript', langColor: 'var(--lang-js)', host: 'github',
      desc: 'Tree-sitter grammar for ZenStack ZModel files (.zmodel).',
      branch: 'main', ahead: 0, behind: 0, dirty: 0, status: 'stale',
      stars: 18, issues: 1, release: 'v0.1.0', commitAgo: '5mo ago', stale: true, fav: false,
      ai: 'Dormant grammar project. No commits in 5 months — likely feature-complete or paused.',
      commits: [
        { msg: 'feat: initial grammar + highlights', meta: 'you · 5mo ago' },
      ],
    },
    {
      name: 'desktop-app', slug: 'Hankanman/desktop-app', path: '~/dev/ma-desktop', root: 'dev',
      lang: 'TypeScript', langColor: 'var(--lang-ts)', host: 'github',
      desc: 'Official companion desktop app for Music Assistant.',
      branch: 'main', ahead: 0, behind: 3, dirty: 0, status: 'behind',
      stars: 74, issues: 5, release: 'v2.1.0', commitAgo: '3d ago', stale: false, fav: false,
      ai: 'Behind upstream by 3 commits — a sync/pull is due before the next build.',
      commits: [
        { msg: 'chore: bump electron 30', meta: 'upstream · 3d ago' },
      ],
    },
    {
      name: 'KARA', slug: 'Hankanman/KARA', path: '~/dev/kara', root: 'dev',
      lang: 'Python', langColor: 'var(--lang-python)', host: 'github',
      desc: 'Knowledge-assisted retrieval agent. Experimental.',
      branch: 'main', ahead: 0, behind: 0, dirty: 1, status: 'dirty',
      stars: 6, issues: 0, release: null, commitAgo: '3w ago', stale: false, fav: false,
      ai: 'Early experiment with one stray uncommitted change. Sparse history.',
      commits: [
        { msg: 'spike: vector store wiring', meta: 'you · 3w ago' },
      ],
    },
    {
      name: 'Accessible-Apps-Kit', slug: 'Trustmarque/Accessible-Apps-Kit', path: '~/work/trustmarque/aak', root: 'work',
      lang: 'TypeScript', langColor: 'var(--lang-ts)', host: 'gitlab',
      desc: 'Tools to help build accessible experiences in the Power Platform.',
      branch: 'develop', ahead: 0, behind: 0, dirty: 0, status: 'clean',
      stars: 31, issues: 9, release: 'v3.4.0', commitAgo: '1d ago', stale: false, fav: false,
      ai: 'Team project on a self-hosted GitLab. Clean working tree; steady issue flow on develop.',
      commits: [
        { msg: 'feat: contrast checker control', meta: 'team · 1d ago' },
      ],
    },
    {
      name: 'PowerDocu', slug: 'Trustmarque/PowerDocu', path: '~/work/trustmarque/powerdocu', root: 'work',
      lang: 'C#', langColor: '#178600', host: 'gitlab',
      desc: 'Generate technical documentation from Power Automate Flows and Power Apps.',
      branch: 'main', ahead: 0, behind: 0, dirty: 0, status: 'clean',
      stars: 402, issues: 22, release: 'v1.8.2', commitAgo: '4d ago', stale: false, fav: false,
      ai: 'Widely-used internal tool. Stable; most activity is in issue triage rather than code.',
      commits: [
        { msg: 'release: v1.8.2', meta: 'team · 4d ago' },
      ],
    },
  ],

  /* ---- Phase 4: followed / starred browser ---- */
  following: [
    { id: 'tauri', name: 'tauri-apps', initials: 'TA', color: 'var(--lang-rust)' },
    { id: 'ollama', name: 'ollama', initials: 'OL', color: 'var(--clean)' },
    { id: 'shadcn', name: 'shadcn', initials: 'SH', color: 'var(--fg-1)' },
    { id: 'vercel', name: 'vercel', initials: 'V', color: 'var(--fg-0)' },
    { id: 'ggml', name: 'ggml-org', initials: 'GG', color: 'var(--ai)' },
    { id: 'me', name: 'Hankanman', initials: 'HK', color: 'var(--accent)' },
  ],

  // chronological activity stream (newest first), grouped by `day`
  feed: [
    {
      id: 'f1', type: 'release', day: 'Today',
      repo: 'tauri-apps/tauri', actor: 'tauri-apps', initials: 'TA', color: 'var(--lang-rust)',
      lang: 'Rust', time: '2h ago', stars: 84200,
      version: 'v2.3.0', tag: 'Tauri 2.3',
      notes: [
        'feat: tray icon menus on Linux (AppIndicator)',
        'perf: 18% smaller bundle on release builds',
        'fix: IPC payload size limit raised to 256MB',
      ],
    },
    {
      id: 'f2', type: 'release', day: 'Today',
      repo: 'ollama/ollama', actor: 'ollama', initials: 'OL', color: 'var(--clean)',
      lang: 'Go', time: '5h ago', stars: 98400,
      version: 'v0.5.7', tag: 'Ollama 0.5.7',
      notes: [
        'new: structured outputs (JSON schema) for all models',
        'fix: GGUF metadata parsing for quantized weights',
      ],
    },
    {
      id: 'f3', type: 'star', day: 'Today',
      repo: 'Hankanman/Meetily-Local', actor: 'Meetily-Local', initials: 'ML', color: 'var(--lang-rust)',
      lang: 'Rust', time: '6h ago', stars: 842,
      text: 'crossed 800 stars', milestone: '800',
    },
    {
      id: 'f4', type: 'push', day: 'Today',
      repo: 'ggml-org/llama.cpp', actor: 'ggml-org', initials: 'GG', color: 'var(--ai)',
      lang: 'C++', time: '8h ago', stars: 69100,
      branch: 'master', count: 6,
      commits: [
        { msg: 'cuda: fused attention for Gemma-3', meta: '0a1c2f3' },
        { msg: 'metal: faster q4_k dequant', meta: '7b9e1d4' },
        { msg: 'server: /v1/embeddings batching', meta: 'c33a90b' },
      ],
    },
    {
      id: 'f5', type: 'release', day: 'Yesterday',
      repo: 'shadcn-ui/ui', actor: 'shadcn', initials: 'SH', color: 'var(--fg-1)',
      lang: 'TypeScript', time: '1d ago', stars: 74600,
      version: '2025.05', tag: 'Calendar release',
      notes: [
        'new: Sidebar-07 block + collapsible groups',
        'a11y: focus-visible rings across all primitives',
      ],
    },
    {
      id: 'f6', type: 'newrepo', day: 'Yesterday',
      repo: 'vercel/geist-icons', actor: 'vercel', initials: 'V', color: 'var(--fg-0)',
      lang: 'TypeScript', time: '1d ago', stars: 1240,
      desc: 'The official Geist icon set as tree-shakeable React components.',
    },
    {
      id: 'f7', type: 'issue', day: 'Yesterday',
      repo: 'tauri-apps/tauri', actor: 'lorenzolewis', initials: 'LL', color: 'var(--star)',
      lang: 'Rust', time: '1d ago',
      number: 9421, title: 'Wayland: window decorations flicker on resize', state: 'open',
    },
    {
      id: 'f8', type: 'push', day: 'This week',
      repo: 'ollama/ollama', actor: 'ollama', initials: 'OL', color: 'var(--clean)',
      lang: 'Go', time: '3d ago', stars: 98400,
      branch: 'main', count: 11,
      commits: [
        { msg: 'runner: parakeet ASR adapter', meta: 'a91b2c0' },
        { msg: 'docs: device-flow auth guide', meta: 'd40f7e2' },
      ],
    },
    {
      id: 'f9', type: 'star', day: 'This week',
      repo: 'ggml-org/whisper.cpp', actor: 'whisper.cpp', initials: 'WC', color: 'var(--ai)',
      lang: 'C++', time: '4d ago', stars: 38900,
      text: 'crossed 38k stars', milestone: '38k',
    },
    {
      id: 'f10', type: 'release', day: 'This week',
      repo: 'Hankanman/Changelog-Weaver', actor: 'Changelog-Weaver', initials: 'CW', color: 'var(--lang-python)',
      lang: 'Python', time: '5d ago', stars: 213,
      version: 'v2.0.0', tag: 'GitLab support',
      notes: [
        'new: GitLab (incl. self-hosted) provider',
        'new: per-section AI summaries',
      ],
    },
  ],
};
