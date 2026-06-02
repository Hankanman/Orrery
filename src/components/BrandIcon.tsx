import { Bot, Code, SquareTerminal, type LucideIcon } from "lucide-react";
import { BRAND_PATHS } from "@/lib/brand-icons";
import { cn } from "@/lib/utils";

type Category = "ide" | "terminal" | "agent";

const FALLBACK: Record<Category, LucideIcon> = {
  ide: Code,
  terminal: SquareTerminal,
  agent: Bot,
};

interface BrandIconProps {
  /** Brand id into BRAND_PATHS (e.g. "vscode", "kitty", "claude"). */
  brand: string;
  /** Used to pick the generic glyph when no brand logo exists. */
  category: Category;
  className?: string;
}

/** A brand logo (monochrome, currentColor) with a per-category fallback glyph
 *  for tools simple-icons doesn't cover. */
export function BrandIcon({ brand, category, className }: BrandIconProps) {
  const path = BRAND_PATHS[brand];
  if (!path) {
    const Glyph = FALLBACK[category];
    return <Glyph className={cn("size-3.5", className)} aria-hidden />;
  }
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" className={cn("size-3.5", className)} aria-hidden>
      <path d={path} />
    </svg>
  );
}
