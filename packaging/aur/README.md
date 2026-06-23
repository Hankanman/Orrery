# AUR packaging

[`orrery-git`](PKGBUILD) builds Orrery from the latest `main`. There's no tagged
release yet, so this VCS package is the way to install on Arch.

The `.desktop` entry lives here and is installed from the cloned source, so the
**AUR repository only needs `PKGBUILD` and `.SRCINFO`** — everything else comes
from the GitHub clone.

## Test the build locally

On Arch (or in a container), from a copy of this directory:

```sh
makepkg -si          # build + install
makepkg -f --printsrcinfo > .SRCINFO   # regenerate metadata after edits
namcap PKGBUILD *.pkg.tar.zst          # optional lint
```

Not on Arch? Build it in a container (this is how it's validated in CI):

```sh
podman run --rm -v "$PWD:/pkg:ro" archlinux:latest bash -c '
  pacman -Syu --noconfirm --needed base-devel git rust cmake pkgconf \
    vulkan-headers vulkan-icd-loader fontconfig libxkbcommon hicolor-icon-theme
  useradd -m b && cp /pkg/PKGBUILD /home/b/ && chown -R b /home/b
  su b -c "cd ~ && makepkg -f --noconfirm --skipinteg --nodeps"
'
```

## Publish / update on the AUR

The AUR is a git remote keyed to your SSH key (set up an account + key at
<https://aur.archlinux.org> first):

```sh
git clone ssh://aur@aur.archlinux.org/orrery-git.git
cd orrery-git
cp /path/to/Orrery/packaging/aur/PKGBUILD .
makepkg --printsrcinfo > .SRCINFO     # required; AUR rejects pushes without it
git add PKGBUILD .SRCINFO
git commit -m "orrery-git: initial import"   # or "update to <pkgver>"
git push
```

Bump `pkgrel` when only the PKGBUILD changes; `pkgver()` tracks `main`
automatically, so a routine "update" is just rebuilding `.SRCINFO` and pushing.

## Notes

- `--no-bundle` compiles just the binary; the package installs the binary,
  `.desktop`, icons, and `LICENSE` itself (no deb/AppImage step, so the
  `linuxdeploy`/`NO_STRIP` AppImage issue doesn't apply here).
- A future tagged release can add a non-VCS `orrery` (source tarball) and/or
  `orrery-bin` (prebuilt) alongside this.
