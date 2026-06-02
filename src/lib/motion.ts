// Reduce-motion preference. A pure UI setting (no backend), applied as a class
// on <html> so CSS can near-instantly disable animations/transitions — useful
// where the webview can't GPU-accelerate them smoothly (e.g. WebKitGTK on
// NVIDIA). Independent of, and additive to, the OS prefers-reduced-motion.
const KEY = "orr.reduceMotion";

export function reduceMotionEnabled(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

/** Apply the stored preference to <html>. Call once at startup, before paint. */
export function initReduceMotion(): void {
  if (reduceMotionEnabled()) document.documentElement.classList.add("reduce-motion");
}

/** Persist and apply the preference live. */
export function setReduceMotion(on: boolean): void {
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // ignore — private mode / no storage
  }
  document.documentElement.classList.toggle("reduce-motion", on);
}
