import { useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { Check, Copy, Dices } from "lucide-react";
import * as T from "@/lib/tools";
import { cn } from "@/lib/utils";

/** Sidebar groupings for the Dev Tools page (ordered as shown). */
export const TOOL_CATEGORIES = ["Generate", "Encode", "Data", "Convert", "Text"] as const;
export type ToolCategory = (typeof TOOL_CATEGORIES)[number];

export interface DevTool {
  id: string;
  name: string;
  description: string;
  category: ToolCategory;
  keywords: string[];
  Component: ComponentType;
}

// ── shared bits ───────────────────────────────────────────────────────────────

const AREA =
  "w-full resize-y rounded-md border border-border bg-background/50 p-2 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-primary/50";

function Area(props: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea spellCheck={false} rows={4} {...props} className={cn(AREA, props.className)} />;
}

function CopyButton({ value }: { value: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground disabled:opacity-40"
      disabled={!value}
      onClick={() => {
        navigator.clipboard?.writeText(value).catch(() => {});
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
    >
      {done ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      {done ? "Copied" : "Copy"}
    </button>
  );
}

/** Read-only result row with a copy button; shows an error in danger colour. */
function Result({ value, error, mono = true }: { value: string; error?: string; mono?: boolean }) {
  if (error) return <p className="text-xs text-danger">{error}</p>;
  if (!value) return null;
  return (
    <div className="flex items-start gap-2">
      <pre className={cn("min-w-0 flex-1 overflow-x-auto whitespace-pre-wrap break-words rounded-md border border-border bg-background/50 p-2 text-xs", mono && "font-mono")}>
        {value}
      </pre>
      <CopyButton value={value} />
    </div>
  );
}

function Seg<V extends string>({ value, onChange, options }: { value: V; onChange: (v: V) => void; options: { v: V; label: string }[] }) {
  return (
    <div className="orr-seg text">
      {options.map((o) => (
        <button key={o.v} type="button" className={cn(value === o.v && "on")} onClick={() => onChange(o.v)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Run a transform that may throw, returning [output, error]. */
function safe(fn: () => string): [string, string | undefined] {
  try {
    return [fn(), undefined];
  } catch (e) {
    return ["", (e as Error).message];
  }
}

function Wrap({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-2">{children}</div>;
}

// ── tools ─────────────────────────────────────────────────────────────────────

function UuidTool() {
  const [mode, setMode] = useState<"v4" | "v7">("v4");
  const [value, setValue] = useState(() => T.uuidv4());
  const gen = () => setValue(mode === "v4" ? T.uuidv4() : T.uuidv7());
  return (
    <Wrap>
      <div className="flex items-center gap-2">
        <Seg value={mode} onChange={setMode} options={[{ v: "v4", label: "v4" }, { v: "v7", label: "v7" }]} />
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:text-foreground"
          onClick={gen}
        >
          <Dices className="size-3.5" /> Generate
        </button>
      </div>
      <Result value={value} />
    </Wrap>
  );
}

function makeTwoWay(encode: (s: string) => string, decode: (s: string) => string, encodeLabel = "Encode", decodeLabel = "Decode") {
  return function TwoWay() {
    const [mode, setMode] = useState<"enc" | "dec">("enc");
    const [input, setInput] = useState("");
    const [out, err] = useMemo<[string, string | undefined]>(
      () => (input ? safe(() => (mode === "enc" ? encode(input) : decode(input))) : ["", undefined]),
      [input, mode],
    );
    return (
      <Wrap>
        <Seg value={mode} onChange={setMode} options={[{ v: "enc", label: encodeLabel }, { v: "dec", label: decodeLabel }]} />
        <Area value={input} onChange={(e) => setInput(e.target.value)} placeholder={mode === "enc" ? "Plain text…" : "Encoded text…"} />
        <Result value={out} error={err} />
      </Wrap>
    );
  };
}

function JsonTool() {
  const [input, setInput] = useState("");
  const [indent, setIndent] = useState<"2" | "min">("2");
  const [out, err] = useMemo<[string, string | undefined]>(
    () => (input ? safe(() => (indent === "min" ? T.minifyJson(input) : T.formatJson(input, 2))) : ["", undefined]),
    [input, indent],
  );
  return (
    <Wrap>
      <Seg value={indent} onChange={setIndent} options={[{ v: "2", label: "Pretty" }, { v: "min", label: "Minify" }]} />
      <Area value={input} onChange={(e) => setInput(e.target.value)} placeholder='{"paste":"json here"}' />
      <Result value={out} error={err} />
    </Wrap>
  );
}

function JwtTool() {
  const [input, setInput] = useState("");
  const decoded = useMemo(() => {
    if (!input.trim()) return null;
    try {
      const d = T.decodeJwt(input);
      return { header: JSON.stringify(d.header, null, 2), payload: JSON.stringify(d.payload, null, 2), error: undefined };
    } catch (e) {
      return { error: (e as Error).message } as { error: string; header?: string; payload?: string };
    }
  }, [input]);
  return (
    <Wrap>
      <Area value={input} onChange={(e) => setInput(e.target.value)} placeholder="eyJhbGci…  (decode only — no signature check)" />
      {decoded?.error && <p className="text-xs text-danger">{decoded.error}</p>}
      {decoded?.header && (
        <>
          <span className="text-xs text-muted-foreground">Header</span>
          <Result value={decoded.header} />
          <span className="text-xs text-muted-foreground">Payload</span>
          <Result value={decoded.payload!} />
        </>
      )}
    </Wrap>
  );
}

const HASH_ALGOS = ["SHA-1", "SHA-256", "SHA-384", "SHA-512"] as const;
function HashTool() {
  const [input, setInput] = useState("");
  const [algo, setAlgo] = useState<(typeof HASH_ALGOS)[number]>("SHA-256");
  const [digest, setDigest] = useState("");
  useEffect(() => {
    if (!input) {
      setDigest("");
      return;
    }
    let cancelled = false;
    crypto.subtle
      .digest(algo, new TextEncoder().encode(input))
      .then((buf) => {
        if (cancelled) return;
        setDigest(Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, "0")).join(""));
      })
      .catch(() => setDigest(""));
    return () => {
      cancelled = true;
    };
  }, [input, algo]);
  return (
    <Wrap>
      <Seg value={algo} onChange={setAlgo} options={HASH_ALGOS.map((a) => ({ v: a, label: a.replace("SHA-", "") }))} />
      <Area value={input} onChange={(e) => setInput(e.target.value)} placeholder="Text to hash…" />
      <Result value={digest} />
    </Wrap>
  );
}

function TimestampTool() {
  const [input, setInput] = useState("");
  const parsed = useMemo(() => {
    const raw = input.trim();
    if (!raw) return null;
    let date: Date;
    if (/^\d+$/.test(raw)) {
      // bare number → epoch; <= 11 digits is seconds, else ms
      const n = Number(raw);
      date = new Date(raw.length > 11 ? n : n * 1000);
    } else {
      date = new Date(raw);
    }
    if (Number.isNaN(date.getTime())) return { error: "Unrecognised date or timestamp" };
    return {
      lines: [
        `ISO 8601   ${date.toISOString()}`,
        `UTC        ${date.toUTCString()}`,
        `Local      ${date.toString()}`,
        `Epoch (s)  ${Math.floor(date.getTime() / 1000)}`,
        `Epoch (ms) ${date.getTime()}`,
      ].join("\n"),
    };
  }, [input]);
  return (
    <Wrap>
      <div className="flex items-center gap-2">
        <button
          type="button"
          className="rounded-md border border-border px-2 py-1 text-xs hover:text-foreground"
          onClick={() => setInput(String(Math.floor(Date.now() / 1000)))}
        >
          Now
        </button>
        <span className="text-xs text-muted-foreground">epoch (s/ms) or any date string</span>
      </div>
      <input
        className="w-full rounded-md border border-border bg-background/50 p-2 font-mono text-xs outline-none focus:border-primary/50"
        value={input}
        spellCheck={false}
        onChange={(e) => setInput(e.target.value)}
        placeholder="1717000000"
      />
      <Result value={parsed && !("error" in parsed) ? parsed.lines : ""} error={parsed && "error" in parsed ? parsed.error : undefined} />
    </Wrap>
  );
}

function NumberBaseTool() {
  const [input, setInput] = useState("");
  const [base, setBase] = useState<T.NumBase>("dec");
  const result = useMemo(() => {
    if (!input.trim()) return null;
    try {
      const v = T.parseToBigInt(input, base);
      return { lines: T.NUM_BASES.map((b) => `${b.label.padEnd(8)} ${T.formatBigInt(v, b.key)}`).join("\n") };
    } catch (e) {
      return { error: (e as Error).message };
    }
  }, [input, base]);
  return (
    <Wrap>
      <Seg value={base} onChange={setBase} options={T.NUM_BASES.map((b) => ({ v: b.key, label: b.label }))} />
      <input
        className="w-full rounded-md border border-border bg-background/50 p-2 font-mono text-xs outline-none focus:border-primary/50"
        value={input}
        spellCheck={false}
        onChange={(e) => setInput(e.target.value)}
        placeholder={base === "hex" ? "ff" : base === "bin" ? "1010" : "255"}
      />
      <Result value={result && !("error" in result) ? result.lines : ""} error={result && "error" in result ? result.error : undefined} />
    </Wrap>
  );
}

function ColorTool() {
  const [input, setInput] = useState("#1dd3c4");
  const { hex, text, error } = useMemo(() => {
    try {
      const rgb = T.hexToRgb(input);
      const hsl = T.rgbToHsl(rgb);
      return {
        hex: T.rgbToHex(rgb),
        text: `RGB  rgb(${rgb.r}, ${rgb.g}, ${rgb.b})\nHSL  hsl(${hsl.h}, ${hsl.s}%, ${hsl.l}%)`,
        error: undefined as string | undefined,
      };
    } catch (e) {
      return { hex: "", text: "", error: (e as Error).message };
    }
  }, [input]);
  return (
    <Wrap>
      <div className="flex items-center gap-2">
        <span className="size-8 shrink-0 rounded-md border border-border" style={{ background: hex || "transparent" }} />
        <input
          className="w-full rounded-md border border-border bg-background/50 p-2 font-mono text-xs outline-none focus:border-primary/50"
          value={input}
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          placeholder="#1dd3c4"
        />
      </div>
      <Result value={text} error={error} />
    </Wrap>
  );
}

const CASES: { label: string; fn: (s: string) => string }[] = [
  { label: "camelCase", fn: T.toCamelCase },
  { label: "PascalCase", fn: T.toPascalCase },
  { label: "snake_case", fn: T.toSnakeCase },
  { label: "kebab-case", fn: T.toKebabCase },
  { label: "CONSTANT_CASE", fn: T.toConstantCase },
  { label: "Title Case", fn: T.toTitleCase },
  { label: "slug", fn: T.slugify },
];
function CaseTool() {
  const [input, setInput] = useState("");
  return (
    <Wrap>
      <Area value={input} onChange={(e) => setInput(e.target.value)} rows={2} placeholder="Some text to convert" />
      <div className="flex flex-col gap-1.5">
        {CASES.map(({ label, fn }) => {
          const v = input ? fn(input) : "";
          return (
            <div key={label} className="flex items-center gap-2">
              <span className="w-28 shrink-0 text-xs text-muted-foreground">{label}</span>
              <code className="min-w-0 flex-1 truncate text-xs">{v}</code>
              <CopyButton value={v} />
            </div>
          );
        })}
      </div>
    </Wrap>
  );
}

function RegexTool() {
  const [pattern, setPattern] = useState("");
  const [flags, setFlags] = useState("g");
  const [text, setText] = useState("");
  const result = useMemo(() => {
    if (!pattern || !text) return null;
    try {
      const re = new RegExp(pattern, flags.includes("g") ? flags : flags + "g");
      const matches = [...text.matchAll(re)];
      if (matches.length === 0) return { lines: "No matches." };
      return {
        lines: matches
          .map((m, i) => {
            const groups = m.length > 1 ? `  groups: [${m.slice(1).map((g) => JSON.stringify(g)).join(", ")}]` : "";
            return `${i + 1}. ${JSON.stringify(m[0])} @ ${m.index}${groups}`;
          })
          .join("\n"),
      };
    } catch (e) {
      return { error: (e as Error).message };
    }
  }, [pattern, flags, text]);
  return (
    <Wrap>
      <div className="flex items-center gap-2">
        <input
          className="min-w-0 flex-1 rounded-md border border-border bg-background/50 p-2 font-mono text-xs outline-none focus:border-primary/50"
          value={pattern}
          spellCheck={false}
          onChange={(e) => setPattern(e.target.value)}
          placeholder="pattern"
        />
        <input
          className="w-16 rounded-md border border-border bg-background/50 p-2 font-mono text-xs outline-none focus:border-primary/50"
          value={flags}
          spellCheck={false}
          onChange={(e) => setFlags(e.target.value)}
          placeholder="flags"
        />
      </div>
      <Area value={text} onChange={(e) => setText(e.target.value)} placeholder="Test string…" />
      <Result value={result && !("error" in result) ? result.lines : ""} error={result && "error" in result ? result.error : undefined} />
    </Wrap>
  );
}

// ── registry ──────────────────────────────────────────────────────────────────

export const TOOLS: DevTool[] = [
  { id: "uuid", name: "UUID generator", description: "Random v4 or time-ordered v7 UUIDs.", category: "Generate", keywords: ["uuid", "guid", "id", "v4", "v7"], Component: UuidTool },
  { id: "hash", name: "Hash (SHA)", description: "SHA-1/256/384/512 of some text.", category: "Generate", keywords: ["hash", "sha", "sha256", "digest", "checksum"], Component: HashTool },
  { id: "url", name: "URL encode / decode", description: "Percent-encode or decode a string.", category: "Encode", keywords: ["url", "uri", "percent", "encode", "decode", "escape"], Component: makeTwoWay(T.encodeUrl, T.decodeUrl) },
  { id: "base64", name: "Base64 encode / decode", description: "UTF-8 safe Base64 both ways.", category: "Encode", keywords: ["base64", "encode", "decode", "atob", "btoa"], Component: makeTwoWay(T.encodeBase64, T.decodeBase64) },
  { id: "json", name: "JSON format / minify", description: "Pretty-print, minify, and validate JSON.", category: "Data", keywords: ["json", "format", "pretty", "minify", "validate", "beautify"], Component: JsonTool },
  { id: "jwt", name: "JWT decoder", description: "Decode a JWT's header and payload (no verify).", category: "Data", keywords: ["jwt", "token", "decode", "auth", "bearer"], Component: JwtTool },
  { id: "timestamp", name: "Timestamp converter", description: "Unix epoch ↔ human dates.", category: "Convert", keywords: ["timestamp", "epoch", "unix", "date", "time"], Component: TimestampTool },
  { id: "numbase", name: "Number base converter", description: "Convert between dec, hex, binary, octal.", category: "Convert", keywords: ["number", "base", "hex", "binary", "octal", "radix", "convert"], Component: NumberBaseTool },
  { id: "color", name: "Colour converter", description: "HEX ↔ RGB ↔ HSL, with a swatch.", category: "Convert", keywords: ["color", "colour", "hex", "rgb", "hsl", "swatch"], Component: ColorTool },
  { id: "case", name: "Case converter", description: "camel, snake, kebab, slug, and more.", category: "Text", keywords: ["case", "camel", "snake", "kebab", "slug", "pascal", "constant"], Component: CaseTool },
  { id: "regex", name: "Regex tester", description: "Test a pattern and inspect match groups.", category: "Text", keywords: ["regex", "regexp", "pattern", "match", "test"], Component: RegexTool },
];
