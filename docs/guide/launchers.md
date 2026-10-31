# Launchers

Every repo card is a launchpad. Orrery has four launch actions, each with its own colour identity, and a one-click preset switcher in Settings so you never have to hand-write a command.

## The four actions

| Action | What it does |
|---|---|
| **Open in IDE** | Opens the repo in your configured editor. |
| **Agent** | Drops a terminal coding agent into the repo (Claude Code, Aider, Codex, …). |
| **Folder** | Reveals the repo's directory in your file manager. |
| **Open on host** | Opens the repo's GitHub/GitLab page in your browser (when it has a remote). |

The IDE and Agent buttons show the configured tool's logo and name (e.g. *VS Code*, *Claude Code*) and stay neutral so the brand reads cleanly; Folder and Open-on-host carry their own accent colours.

## Preset switcher

In **Settings → Launchers**, pick your editor and agent from preset chips with real brand logos — no command syntax required.

![Settings — launcher presets](/shots/settings-launchers.png)

- **Open in IDE** — VS Code, VSCodium, Cursor, Windsurf, Zed, Sublime Text, Fleet, Lapce, the JetBrains family (IntelliJ, WebStorm, PyCharm, GoLand, CLion, Rider, RustRover, PhpStorm), Emacs, GNOME Builder, Kate, Geany.
- **Terminal agent** — two axes: a **terminal emulator** (Kitty, Alacritty, Ghostty, WezTerm, Foot, Konsole, GNOME Terminal, xterm) and an **agent CLI** (Claude Code, Codex, Gemini CLI, Aider, OpenCode, Cursor Agent, Crush, Qwen Code). Picking a terminal preserves your agent and vice-versa.

The active preset highlights in the action's colour. Detection round-trips, so the chips reflect whatever the saved command resolves to.

## Custom commands

Prefer to hand-roll it? The raw command field stays available under each preset row. Use `{path}` where the repo path should be substituted, e.g.:

```text
code {path}
kitty --working-directory {path} -e claude
```

Any command that doesn't match a preset simply shows no highlighted chip and falls back to a generic glyph on the cards.

## Search from the desktop launcher

While Orrery is running (including hidden in the tray), your repos are searchable from the desktop's own launcher. Type at least two characters of a repo's name, its `owner/name` slug or its path; several words must all match, and name matches rank first.

### KDE Plasma (KRunner)

Repos show up in KRunner and the Plasma launcher's search. Orrery registers itself the first time it runs (it writes `~/.local/share/krunner/dbusplugins/orrery.desktop`), so nothing needs installing. Choosing a result opens it in your IDE. Hover a result, or select it and press <kbd>Tab</kbd>, for its other actions:

| Action | What it does |
|---|---|
| **Open terminal agent** | Runs your configured terminal agent in the repo. |
| **Open folder** | Opens the repo's folder in the file manager. |
| **Copy path** | Copies the repo's full path to the clipboard. |

If results don't appear straight away, restart KRunner (`kquitapp6 krunner`) or log out and back in, so it picks up the new plugin.

### GNOME Shell

Repos show up in the Activities overview search. Clicking a result opens it in your IDE. GNOME reads search providers only from system directories, so this needs the `.deb`, `.rpm` or AUR package, which install `orrery-search-provider.ini` under `/usr/share/gnome-shell/search-providers/`. With the install script or the AppImage, copy it there once (the install script prints the command), then log out and back in:

```sh
sudo install -Dm644 packaging/orrery-search-provider.ini \
  /usr/local/share/gnome-shell/search-providers/orrery-search-provider.ini
```

Orrery is listed under **Settings → Search**, where you can turn it off or reorder it.
