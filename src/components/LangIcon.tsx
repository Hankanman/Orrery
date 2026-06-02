import { LANG_LOGOS } from "@/lib/lang-icons";
import { languageColor } from "@/lib/format";
import { cn } from "@/lib/utils";

interface LangIconProps {
  language: string | null;
  className?: string;
}

/** A full-colour programming-language logo. Multi-colour marks render with
 *  their own fills; colour-less marks (e.g. Rust) inherit the language colour
 *  via the root `fill`. Falls back to the classic coloured dot when there's no
 *  logo (or no language). */
export function LangIcon({ language, className }: LangIconProps) {
  const logo = language ? LANG_LOGOS[language] : undefined;
  if (!logo) {
    const color = languageColor(language);
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
      viewBox={logo.vb}
      fill={languageColor(language)}
      className={cn("size-3.5 shrink-0", className)}
      role="img"
      aria-label={language ?? undefined}
      dangerouslySetInnerHTML={{ __html: logo.svg }}
    />
  );
}
