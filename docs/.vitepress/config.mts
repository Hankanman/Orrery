import { defineConfig } from "vitepress";

// Project Pages site → served under /Orrery/.
export default defineConfig({
  title: "Orrery",
  description: "every repo in orbit — a Linux-native command center for your git repos",
  base: "/Orrery/",
  lang: "en-US",
  cleanUrls: true,
  lastUpdated: true,
  // The design-system page links into the source tree (../../src/...) — those
  // are meant to be read on GitHub, not as site pages. Ignore them; real
  // cross-page links are still validated.
  ignoreDeadLinks: [/\.\.\/\.\.\/src\//, /^\.\.\/\.\.\//],
  head: [
    ["link", { rel: "icon", href: "/Orrery/logo.svg" }],
    ["meta", { name: "theme-color", content: "#1dd3c4" }],
  ],
  themeConfig: {
    logo: "/logo.svg",
    nav: [
      { text: "Guide", link: "/guide/introduction" },
      { text: "Features", link: "/guide/mission-control" },
      {
        text: "Internals",
        items: [
          { text: "Design system", link: "/design-system/" },
          { text: "Rendering performance", link: "/rendering-performance" },
        ],
      },
      { text: "GitHub", link: "https://github.com/Hankanman/Orrery" },
    ],
    sidebar: [
      {
        text: "Guide",
        collapsed: false,
        items: [
          { text: "Introduction", link: "/guide/introduction" },
          { text: "Getting started", link: "/guide/getting-started" },
          { text: "Configuration", link: "/guide/configuration" },
        ],
      },
      {
        text: "Feature tour",
        collapsed: false,
        items: [
          { text: "Mission Control", link: "/guide/mission-control" },
          { text: "Launchers", link: "/guide/launchers" },
          { text: "Inbox, Feed & Explore", link: "/guide/inbox-feed-explore" },
          { text: "Local AI", link: "/guide/local-ai" },
        ],
      },
      {
        text: "Internals",
        collapsed: true,
        items: [
          { text: "Design system", link: "/design-system/" },
          { text: "Rendering performance", link: "/rendering-performance" },
        ],
      },
    ],
    socialLinks: [{ icon: "github", link: "https://github.com/Hankanman/Orrery" }],
    search: { provider: "local" },
    editLink: {
      pattern: "https://github.com/Hankanman/Orrery/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },
    footer: {
      message: "Released under the MIT License.",
      copyright: "© 2026 Seb Burrell",
    },
  },
});
