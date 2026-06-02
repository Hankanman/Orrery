import { LANG_PATHS } from "@/lib/lang-icons";
import { languageColor } from "@/lib/format";
import { cn } from "@/lib/utils";

interface LangIconProps {
  language: string | null;
  className?: string;
}

/** A programming-language logo tinted with the language's colour. Falls back to
 *  the classic coloured dot for languages without a logo (or none at all). */
export function LangIcon({ language, className }: LangIconProps) {
  const color = languageColor(language);
  const path = language ? LANG_PATHS[language] : undefined;
  if (!path) {
    return (
      <span
        className={cn("ldot", className)}
        style={{ background: color, color }}
        aria-label={language ?? undefined}
      />
    );
  }
  return (
    <svg
      viewBox="0 0 24 24"
      fill={color}
      className={cn("size-3.5 shrink-0", className)}
      role="img"
      aria-label={language ?? undefined}
    >
      <path d={path} />
    </svg>
  );
}
