# ryucad

CAD built for AI agents to drive, with a from-scratch B-rep kernel (**ryukei**) designed for large-assembly performance. The long-term target is Parasolid-level capability.

- `crates/ryukei`: geometry kernel (single bodies: geometry, topology, booleans, blends, tessellation)
- `crates/ryucad`: application layer (documents, assemblies, instancing, persistence, agent interface)
- Design documents and roadmaps live in the Coxswain planning project `ryucad-ryukei`, not in this repo

## Install

Prebuilt binaries for Linux (x86_64 and aarch64), macOS (Apple silicon) and Windows (x64) are published with each release: the `ryucad` command (static on Linux) and the desktop app, `ryucad-app` (on Linux it needs glibc 2.35 or newer and a desktop). The installers put both in `~/.ryucad/bin`, add it to your PATH, and let you open a project folder in the app from your desktop: "Open in ryucad" on folders in Explorer, an application entry on Linux that also opens `ryucad.json`, and `~/Applications/ryucad.app` on macOS (drop a folder on it, or `open -a ryucad <folder>`). Running one again updates.

While the repository is private, the installers need a GitHub token: they use `GH_TOKEN` or `GITHUB_TOKEN`, or ask `gh` (`gh auth login` first).

```sh
# Linux, macOS and WSL
gh release download -R Seiraiyu/ryucad -p install.sh -O - | sh
```

```powershell
# Windows (PowerShell)
gh release download -R Seiraiyu/ryucad -p install.ps1 -O install.ps1; Get-Content -Raw install.ps1 | iex
```

Once the repository is public, no token is needed:

```sh
curl -fsSL https://github.com/Seiraiyu/ryucad/releases/latest/download/install.sh | sh
```

```powershell
irm https://github.com/Seiraiyu/ryucad/releases/latest/download/install.ps1 | iex
```

Set `RYUCAD_VERSION=v0.1.0` to install a particular version, `RYUCAD_HOME` to install somewhere other than `~/.ryucad`, `RYUCAD_NO_MODIFY_PATH=1` to leave your shell profile alone, and `RYUCAD_NO_APP=1` to install the command only (servers, CI). `ryucad open [folder]` opens a project in the app, or brings its window forward.

To uninstall, run `ryucad daemon stop --all`, delete `~/.ryucad` (`%USERPROFILE%\.ryucad` on Windows), and remove the PATH line the installer added (marked `# added by the ryucad installer`) or the entry in your Windows user PATH. Then remove what lets the desktop open projects: on Linux `~/.local/share/applications/ryucad.desktop` and `~/.local/share/mime/packages/ryucad.xml` (then `update-mime-database ~/.local/share/mime`), on macOS `~/Applications/ryucad.app`, and on Windows the keys `HKCU\Software\Classes\Directory\shell\ryucad` and `HKCU\Software\Classes\Directory\Background\shell\ryucad`.

## Releasing

Merge, then push a tag `vX.Y.Z` (or `vX.Y.Z-rc.1` for a prerelease). The version comes from the tag: the repo's workspace version stays `0.0.0`, and the workflow stamps the tag's version in before building. The `Release` workflow builds and tests every target, publishes the archives with `SHA256SUMS` and both installers, and then tests the installers against the new release.
