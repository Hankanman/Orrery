import { useState } from "react";
import { ChevronDown } from "lucide-react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

const CUSTOM = "__custom__";

interface ModelSelectProps {
  id?: string;
  value: string;
  /** Installed model names queried from Ollama (ai_status). */
  models: string[];
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
}

/**
 * Pick an Ollama model from the installed list, or type a custom one (e.g. a
 * model you'll `ollama pull`). A native <select> styled to match Input — no
 * extra dependency. The current value always stays selectable even if it isn't
 * installed yet.
 */
export function ModelSelect({ id, value, models, onChange, disabled, placeholder }: ModelSelectProps) {
  // Drop into free-text mode either by choice, or when the value isn't a known
  // model and there are models to choose from (so a custom value is editable).
  const [custom, setCustom] = useState(false);
  const known = models.includes(value);
  const isCustom = custom || (models.length > 0 && value !== "" && !known);

  if (isCustom || models.length === 0) {
    return (
      <div className="flex items-center gap-2">
        <Input
          id={id}
          list={undefined}
          spellCheck={false}
          placeholder={placeholder}
          value={value}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
        />
        {models.length > 0 && (
          <button
            type="button"
            className="shrink-0 text-xs text-muted-foreground hover:text-foreground"
            onClick={() => {
              setCustom(false);
              onChange(models[0]);
            }}
          >
            Choose installed
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="relative">
      <select
        id={id}
        value={known ? value : ""}
        disabled={disabled}
        onChange={(e) => {
          if (e.target.value === CUSTOM) {
            setCustom(true);
            onChange("");
          } else {
            onChange(e.target.value);
          }
        }}
        className={cn(
          "h-9 w-full appearance-none rounded-md border border-input bg-transparent px-3 py-1 pr-9 text-sm shadow-xs outline-none transition-[color,box-shadow]",
          "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50",
          "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30",
        )}
      >
        {!value && (
          <option value="" disabled>
            {placeholder ?? "Select a model…"}
          </option>
        )}
        {models.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
        <option value={CUSTOM}>Custom…</option>
      </select>
      <ChevronDown className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
    </div>
  );
}
