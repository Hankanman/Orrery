// Pure helpers behind the Dev Tools page. Kept side-effect-free (no DOM, no
// React) so they're easy to unit-test; the tool components in
// components/tools/registry.tsx wrap these with UI + error handling.

// ── UUID ────────────────────────────────────────────────────────────────────

function formatUuid(b: Uint8Array): string {
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0"));
  return `${h.slice(0, 4).join("")}-${h.slice(4, 6).join("")}-${h.slice(6, 8).join("")}-${h.slice(8, 10).join("")}-${h.slice(10, 16).join("")}`;
}

/** RFC 9562 UUIDv4 (random). */
export function uuidv4(): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40; // version 4
  b[8] = (b[8] & 0x3f) | 0x80; // variant
  return formatUuid(b);
}

/** RFC 9562 UUIDv7 (48-bit ms timestamp + random; sortable). */
export function uuidv7(ms: number = Date.now()): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  const t = BigInt(ms);
  for (let i = 0; i < 6; i++) b[i] = Number((t >> BigInt((5 - i) * 8)) & 0xffn);
  b[6] = (b[6] & 0x0f) | 0x70; // version 7
  b[8] = (b[8] & 0x3f) | 0x80; // variant
  return formatUuid(b);
}

// ── Base64 (UTF-8 safe) ───────────────────────────────────────────────────────

export function encodeBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let bin = "";
  for (const byte of bytes) bin += String.fromCharCode(byte);
  return btoa(bin);
}

export function decodeBase64(b64: string): string {
  const bin = atob(b64.trim());
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

// ── URL ───────────────────────────────────────────────────────────────────────

export const encodeUrl = (s: string): string => encodeURIComponent(s);
export const decodeUrl = (s: string): string => decodeURIComponent(s);

// ── JSON ────────────────────────────────────────────────────────────────────

export function formatJson(text: string, indent = 2): string {
  return JSON.stringify(JSON.parse(text), null, indent);
}
export function minifyJson(text: string): string {
  return JSON.stringify(JSON.parse(text));
}

// ── JWT (decode only — no signature verification) ─────────────────────────────

function b64urlDecode(s: string): string {
  const norm = s.replace(/-/g, "+").replace(/_/g, "/");
  const pad = norm.length % 4 ? 4 - (norm.length % 4) : 0;
  return decodeBase64(norm + "=".repeat(pad));
}

export interface DecodedJwt {
  header: unknown;
  payload: unknown;
}

export function decodeJwt(token: string): DecodedJwt {
  const parts = token.trim().split(".");
  if (parts.length < 2) throw new Error("Not a JWT — expected header.payload.signature");
  return {
    header: JSON.parse(b64urlDecode(parts[0])),
    payload: JSON.parse(b64urlDecode(parts[1])),
  };
}

// ── Number base ───────────────────────────────────────────────────────────────

export type NumBase = "dec" | "hex" | "bin" | "oct";
export const NUM_BASES: { key: NumBase; label: string; radix: number }[] = [
  { key: "dec", label: "Decimal", radix: 10 },
  { key: "hex", label: "Hex", radix: 16 },
  { key: "bin", label: "Binary", radix: 2 },
  { key: "oct", label: "Octal", radix: 8 },
];
const RADIX: Record<NumBase, number> = { dec: 10, hex: 16, bin: 2, oct: 8 };

/** Parse an unsigned integer string in the given base to a BigInt. */
export function parseToBigInt(input: string, base: NumBase): bigint {
  const t = input.trim().toLowerCase().replace(/^0x/, "").replace(/^0b/, "").replace(/^0o/, "").replace(/_/g, "");
  if (t === "") throw new Error("empty input");
  const radix = RADIX[base];
  const digits = "0123456789abcdef".slice(0, radix);
  let v = 0n;
  for (const ch of t) {
    const d = digits.indexOf(ch);
    if (d < 0) throw new Error(`'${ch}' is not a valid ${base} digit`);
    v = v * BigInt(radix) + BigInt(d);
  }
  return v;
}

export const formatBigInt = (v: bigint, base: NumBase): string => v.toString(RADIX[base]);

// ── Colour ────────────────────────────────────────────────────────────────────

export interface Rgb {
  r: number;
  g: number;
  b: number;
}
export interface Hsl {
  h: number;
  s: number;
  l: number;
}

export function hexToRgb(hex: string): Rgb {
  let h = hex.trim().replace(/^#/, "");
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  if (!/^[0-9a-fA-F]{6}$/.test(h)) throw new Error("Expected a hex colour like #1dd3c4");
  return { r: parseInt(h.slice(0, 2), 16), g: parseInt(h.slice(2, 4), 16), b: parseInt(h.slice(4, 6), 16) };
}

export function rgbToHex({ r, g, b }: Rgb): string {
  const c = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

export function rgbToHsl({ r, g, b }: Rgb): Hsl {
  const rn = r / 255, gn = g / 255, bn = b / 255;
  const max = Math.max(rn, gn, bn), min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  let h = 0, s = 0;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === rn) h = (gn - bn) / d + (gn < bn ? 6 : 0);
    else if (max === gn) h = (bn - rn) / d + 2;
    else h = (rn - gn) / d + 4;
    h /= 6;
  }
  return { h: Math.round(h * 360), s: Math.round(s * 100), l: Math.round(l * 100) };
}

export function hslToRgb({ h, s, l }: Hsl): Rgb {
  const hn = h / 360, sn = s / 100, ln = l / 100;
  if (sn === 0) {
    const v = Math.round(ln * 255);
    return { r: v, g: v, b: v };
  }
  const hue = (p: number, q: number, t: number) => {
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  const q = ln < 0.5 ? ln * (1 + sn) : ln + sn - ln * sn;
  const p = 2 * ln - q;
  return {
    r: Math.round(hue(p, q, hn + 1 / 3) * 255),
    g: Math.round(hue(p, q, hn) * 255),
    b: Math.round(hue(p, q, hn - 1 / 3) * 255),
  };
}

// ── Case conversion ───────────────────────────────────────────────────────────

function words(s: string): string[] {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_\-\s]+/g, " ")
    .trim()
    .toLowerCase()
    .split(" ")
    .filter(Boolean);
}
const cap = (w: string) => w.charAt(0).toUpperCase() + w.slice(1);

export const toCamelCase = (s: string): string => words(s).map((w, i) => (i ? cap(w) : w)).join("");
export const toPascalCase = (s: string): string => words(s).map(cap).join("");
export const toSnakeCase = (s: string): string => words(s).join("_");
export const toKebabCase = (s: string): string => words(s).join("-");
export const toConstantCase = (s: string): string => words(s).join("_").toUpperCase();
export const toTitleCase = (s: string): string => words(s).map(cap).join(" ");
export const slugify = (s: string): string =>
  s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
