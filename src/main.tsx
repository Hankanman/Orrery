import React from "react";
import ReactDOM from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
// Geist — the design system's typeface, bundled for offline use.
import "@fontsource/geist-sans";
import "@fontsource/geist-sans/500.css";
import "@fontsource/geist-sans/600.css";
import "@fontsource/geist-mono";
import "@fontsource/geist-mono/500.css";
import { isTauri } from "@/lib/ipc";
import { router } from "@/router";
import "@/index.css";

// Tag the document when running inside the Tauri (WebKitGTK) webview so CSS can
// drop GPU-effects WebKit composites on the CPU (backdrop-filter, fixed
// backgrounds) that judder there but are free in a Chromium browser. Set before
// first paint to avoid a glass→solid flash.
if (isTauri()) document.documentElement.classList.add("tauri");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
);
