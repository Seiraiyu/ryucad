# ryucad

CAD built for AI agents to drive, with a from-scratch B-rep kernel (**ryukei**) designed for large-assembly performance. The long-term target is Parasolid-level capability.

- `crates/ryukei`: geometry kernel (single bodies: geometry, topology, booleans, blends, tessellation)
- `crates/ryucad`: application layer (documents, assemblies, instancing, persistence, agent interface)
- `docs/`: design documents

## Install

Prebuilt binaries for Linux (x86_64 and aarch64, static), macOS (Apple silicon) and Windows (x64) are published with each release. The installers put `ryucad` in `~/.ryucad/bin` and add it to your PATH; running one again updates.

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

Set `RYUCAD_VERSION=v0.1.0` to install a particular version, `RYUCAD_HOME` to install somewhere other than `~/.ryucad`, and `RYUCAD_NO_MODIFY_PATH=1` to leave your shell profile alone.

To uninstall, run `ryucad daemon stop --all`, delete `~/.ryucad` (`%USERPROFILE%\.ryucad` on Windows), and remove the PATH line the installer added (marked `# added by the ryucad installer`) or the entry in your Windows user PATH.

## Releasing

Set `version` in the root `Cargo.toml`, merge, then push a tag `vX.Y.Z` matching it. The `Release` workflow builds and tests every target, publishes the archives with `SHA256SUMS` and both installers, and then tests the installers against the new release.
