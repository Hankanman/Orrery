import type { CSSProperties } from "react";
import { cn } from "@/lib/utils";

/**
 * Brand loading spinner: a satellite circling a glowing core — the Orrery mark
 * in motion. Scales with `size` (px) and follows the accent via --primary.
 * Honors prefers-reduced-motion (the orbit stops; the core still glows).
 */
export function Spinner({ size = 24, className }: { size?: number; className?: string }) {
  return (
    <span
      className={cn("orr-spinner", className)}
      style={{ "--orr-spin-size": `${size}px` } as CSSProperties}
      role="status"
      aria-label="Loading"
    >
      <span className="ring" />
      <span className="sat" />
      <span className="core" />
    </span>
  );
}
