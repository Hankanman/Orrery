import { describe, expect, it } from "vitest";
import {
  decodeBase64,
  decodeJwt,
  decodeUrl,
  encodeBase64,
  encodeUrl,
  formatBigInt,
  formatJson,
  hexToRgb,
  hslToRgb,
  minifyJson,
  parseToBigInt,
  rgbToHex,
  rgbToHsl,
  slugify,
  toCamelCase,
  toConstantCase,
  toKebabCase,
  toPascalCase,
  toSnakeCase,
  toTitleCase,
  uuidv4,
  uuidv7,
} from "./tools";

describe("uuid", () => {
  it("v4 has the right shape and version/variant nibbles", () => {
    const u = uuidv4();
    expect(u).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  });
  it("v7 is version 7 and time-ordered", () => {
    const a = uuidv7(1000);
    const b = uuidv7(2000);
    expect(a[14]).toBe("7");
    // first 48 bits encode the ms timestamp, so earlier sorts first
    expect(a.replace(/-/g, "").slice(0, 12) < b.replace(/-/g, "").slice(0, 12)).toBe(true);
  });
});

describe("base64", () => {
  it("round-trips unicode", () => {
    const s = "héllo 🪐 wörld";
    expect(decodeBase64(encodeBase64(s))).toBe(s);
  });
  it("matches a known value", () => {
    expect(encodeBase64("hello")).toBe("aGVsbG8=");
  });
});

describe("url", () => {
  it("round-trips and encodes reserved chars", () => {
    expect(encodeUrl("a b&c=d")).toBe("a%20b%26c%3Dd");
    expect(decodeUrl("a%20b%26c%3Dd")).toBe("a b&c=d");
  });
});

describe("json", () => {
  it("formats and minifies", () => {
    expect(formatJson('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(minifyJson('{ "a": 1 }')).toBe('{"a":1}');
  });
  it("throws on invalid", () => {
    expect(() => formatJson("{nope}")).toThrow();
  });
});

describe("jwt", () => {
  it("decodes header and payload", () => {
    // {"alg":"HS256","typ":"JWT"} . {"sub":"123","name":"Orrery"} . sig
    const token =
      "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjMiLCJuYW1lIjoiT3JyZXJ5In0.sig";
    const { header, payload } = decodeJwt(token);
    expect(header).toEqual({ alg: "HS256", typ: "JWT" });
    expect(payload).toEqual({ sub: "123", name: "Orrery" });
  });
  it("rejects non-tokens", () => {
    expect(() => decodeJwt("nope")).toThrow();
  });
});

describe("number base", () => {
  it("parses and formats across bases", () => {
    expect(formatBigInt(parseToBigInt("255", "dec"), "hex")).toBe("ff");
    expect(formatBigInt(parseToBigInt("ff", "hex"), "bin")).toBe("11111111");
    expect(formatBigInt(parseToBigInt("0xFF", "hex"), "dec")).toBe("255");
    expect(formatBigInt(parseToBigInt("1010", "bin"), "oct")).toBe("12");
  });
  it("rejects invalid digits", () => {
    expect(() => parseToBigInt("2", "bin")).toThrow();
    expect(() => parseToBigInt("g", "hex")).toThrow();
  });
});

describe("colour", () => {
  it("hex ↔ rgb", () => {
    expect(hexToRgb("#1dd3c4")).toEqual({ r: 29, g: 211, b: 196 });
    expect(rgbToHex({ r: 29, g: 211, b: 196 })).toBe("#1dd3c4");
    expect(hexToRgb("#fff")).toEqual({ r: 255, g: 255, b: 255 });
  });
  it("rgb ↔ hsl round-trips approximately", () => {
    const rgb = { r: 29, g: 211, b: 196 };
    const back = hslToRgb(rgbToHsl(rgb));
    expect(Math.abs(back.r - rgb.r)).toBeLessThanOrEqual(2);
    expect(Math.abs(back.g - rgb.g)).toBeLessThanOrEqual(2);
    expect(Math.abs(back.b - rgb.b)).toBeLessThanOrEqual(2);
  });
  it("rejects bad hex", () => {
    expect(() => hexToRgb("#xyz")).toThrow();
  });
});

describe("case conversion", () => {
  const s = "Orrery devTools page";
  it("converts between cases", () => {
    expect(toCamelCase(s)).toBe("orreryDevToolsPage");
    expect(toPascalCase(s)).toBe("OrreryDevToolsPage");
    expect(toSnakeCase(s)).toBe("orrery_dev_tools_page");
    expect(toKebabCase(s)).toBe("orrery-dev-tools-page");
    expect(toConstantCase(s)).toBe("ORRERY_DEV_TOOLS_PAGE");
    expect(toTitleCase(s)).toBe("Orrery Dev Tools Page");
  });
  it("slugify strips punctuation", () => {
    expect(slugify("Hello, World! (v2)")).toBe("hello-world-v2");
  });
});
