// One-click launch presets for the Settings → Launchers section.
//
// IDEs are GUI editors that take a path directly. The terminal agent is two
// axes — a terminal emulator (with {path}/{agent} placeholders) and an agent
// CLI — composed into the final `{term} … {agent}` command.

export interface IdePreset {
  id: string;
  name: string;
  command: string;
}

export interface TerminalPreset {
  id: string;
  name: string;
  /** Template with {path} and {agent} placeholders. */
  template: string;
}

export interface AgentPreset {
  id: string;
  name: string;
  bin: string;
}

export const IDE_PRESETS: IdePreset[] = [
  { id: "vscode", name: "VS Code", command: "code {path}" },
  { id: "vscodium", name: "VSCodium", command: "codium {path}" },
  { id: "cursor", name: "Cursor", command: "cursor {path}" },
  { id: "windsurf", name: "Windsurf", command: "windsurf {path}" },
  { id: "zed", name: "Zed", command: "zed {path}" },
  { id: "sublime", name: "Sublime Text", command: "subl {path}" },
  { id: "fleet", name: "Fleet", command: "fleet {path}" },
  { id: "lapce", name: "Lapce", command: "lapce {path}" },
  { id: "intellij", name: "IntelliJ IDEA", command: "idea {path}" },
  { id: "webstorm", name: "WebStorm", command: "webstorm {path}" },
  { id: "pycharm", name: "PyCharm", command: "pycharm {path}" },
  { id: "goland", name: "GoLand", command: "goland {path}" },
  { id: "clion", name: "CLion", command: "clion {path}" },
  { id: "rider", name: "Rider", command: "rider {path}" },
  { id: "rustrover", name: "RustRover", command: "rustrover {path}" },
  { id: "phpstorm", name: "PhpStorm", command: "phpstorm {path}" },
  { id: "emacs", name: "Emacs", command: "emacs {path}" },
  { id: "builder", name: "GNOME Builder", command: "gnome-builder {path}" },
  { id: "kate", name: "Kate", command: "kate {path}" },
  { id: "zeditor", name: "Geany", command: "geany {path}" },
];

export const TERMINAL_PRESETS: TerminalPreset[] = [
  { id: "kitty", name: "Kitty", template: "kitty --working-directory {path} -e {agent}" },
  { id: "alacritty", name: "Alacritty", template: "alacritty --working-directory {path} -e {agent}" },
  { id: "ghostty", name: "Ghostty", template: "ghostty --working-directory={path} -e {agent}" },
  { id: "wezterm", name: "WezTerm", template: "wezterm start --cwd {path} -- {agent}" },
  { id: "foot", name: "Foot", template: "foot --working-directory {path} {agent}" },
  { id: "konsole", name: "Konsole", template: "konsole --workdir {path} -e {agent}" },
  { id: "gnome", name: "GNOME Terminal", template: "gnome-terminal --working-directory={path} -- {agent}" },
  { id: "xterm", name: "xterm", template: "xterm -e {agent}" },
];

export const AGENT_PRESETS: AgentPreset[] = [
  { id: "claude", name: "Claude Code", bin: "claude" },
  { id: "codex", name: "Codex", bin: "codex" },
  { id: "gemini", name: "Gemini CLI", bin: "gemini" },
  { id: "aider", name: "Aider", bin: "aider" },
  { id: "opencode", name: "OpenCode", bin: "opencode" },
  { id: "cursor", name: "Cursor Agent", bin: "cursor-agent" },
  { id: "crush", name: "Crush", bin: "crush" },
  { id: "qwen", name: "Qwen Code", bin: "qwen" },
];

/** Which IDE preset (if any) matches the current command. */
export function detectIde(command: string): IdePreset | undefined {
  const c = command.trim();
  return IDE_PRESETS.find((p) => p.command === c);
}

/** Which terminal preset (if any) the agent command uses (by program name). */
export function detectTerminal(command: string): TerminalPreset | undefined {
  const first = command.trim().split(/\s+/)[0];
  return TERMINAL_PRESETS.find((t) => t.template.split(/\s+/)[0] === first);
}

/** Which agent CLI (if any) the agent command runs. */
export function detectAgent(command: string): AgentPreset | undefined {
  return AGENT_PRESETS.find((a) => new RegExp(`(^|\\s)${a.bin}(\\s|$)`).test(command));
}

export function composeAgentCommand(term: TerminalPreset, bin: string): string {
  return term.template.replace("{agent}", bin);
}
