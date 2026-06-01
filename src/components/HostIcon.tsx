import type { Host } from "@/types";
import { cn } from "@/lib/utils";

/**
 * Host brand marks. lucide-react@1.x removed brand icons, so these are inlined:
 * GitHub uses lucide's own stroke path; GitLab is the (filled) tanuki logo.
 */
export function HostIcon({ host, className }: { host: Host; className?: string }) {
  if (host === "github") {
    return (
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        className={cn("size-3.5", className)}
        aria-hidden
      >
        <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
        <path d="M9 18c-4.51 2-5-2-7-2" />
      </svg>
    );
  }
  if (host === "gitlab") {
    return (
      <svg viewBox="0 0 24 24" fill="currentColor" className={cn("size-3.5", className)} aria-hidden>
        <path d="m23.6 9.6-.03-.08-3.3-8.6a.86.86 0 0 0-.85-.55.87.87 0 0 0-.5.2.9.9 0 0 0-.3.43L16.4 6.3H7.6L5.38.99a.84.84 0 0 0-.3-.43.87.87 0 0 0-1.35.34l-3.3 8.6-.04.08a6.1 6.1 0 0 0 2.02 7.05l.02.02 5 3.74 2.47 1.87 1.5 1.14a1 1 0 0 0 1.22 0l1.5-1.14 2.48-1.87 5.02-3.76A6.1 6.1 0 0 0 23.6 9.6Z" />
      </svg>
    );
  }
  return null;
}
