import { describe, expect, it } from "vitest";
import {
  AGENT_PRESETS,
  TERMINAL_PRESETS,
  composeAgentCommand,
  detectAgent,
  detectIde,
  detectTerminal,
} from "./launchers";

describe("detectIde", () => {
  it("matches a known editor command", () => {
    expect(detectIde("code {path}")?.id).toBe("vscode");
    expect(detectIde("  zed {path}  ")?.id).toBe("zed");
  });
  it("returns undefined for a custom command", () => {
    expect(detectIde("my-editor --flag {path}")).toBeUndefined();
  });
});

describe("detectTerminal", () => {
  it("matches by program name", () => {
    expect(detectTerminal("kitty --working-directory {path} -e claude")?.id).toBe("kitty");
    expect(detectTerminal("gnome-terminal --working-directory={path} -- aider")?.id).toBe("gnome");
  });
  it("returns undefined for an unknown terminal", () => {
    expect(detectTerminal("urxvt -e claude")).toBeUndefined();
  });
});

describe("detectAgent", () => {
  it("finds the agent CLI as a whole word", () => {
    expect(detectAgent("kitty --working-directory {path} -e claude")?.id).toBe("claude");
    expect(detectAgent("foot --working-directory {path} cursor-agent")?.id).toBe("cursor");
  });
  it("does not match a substring of another token", () => {
    // "claude-extra" should not be detected as the "claude" agent.
    expect(detectAgent("kitty -e claude-extra")).toBeUndefined();
  });
});

describe("composeAgentCommand", () => {
  it("substitutes the agent into the terminal template", () => {
    const kitty = TERMINAL_PRESETS.find((t) => t.id === "kitty")!;
    expect(composeAgentCommand(kitty, "aider")).toBe("kitty --working-directory {path} -e aider");
  });
  it("round-trips: composed command re-detects its terminal and agent", () => {
    const wez = TERMINAL_PRESETS.find((t) => t.id === "wezterm")!;
    const codex = AGENT_PRESETS.find((a) => a.id === "codex")!;
    const cmd = composeAgentCommand(wez, codex.bin);
    expect(detectTerminal(cmd)?.id).toBe("wezterm");
    expect(detectAgent(cmd)?.id).toBe("codex");
  });
});
