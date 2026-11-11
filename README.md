# taurus

Simple CLI tool for updating/install aur packages.

> [!NOTE]
> Taurus operates only on the AUR, when docs mention package, it is always an aur package.

## Quick start

Install a package

```sh
taurus install -p zen-browser-bin
```

List installed packages

```sh
taurus info
```

Update installed packages

```sh
taurus install
```

Search for a package

```sh
taurus info -p yay
```

## Diff review

When updating/installing a package, a diff is shown (with `git-delta` if installed) always to review what changed.

## Dependency resolution

Not yet implemented

## Examples

```
> taurus info
Getting installed aur packages
(Installed) vesktop-bin 1.6.9-1 UP TO DATE {171} 2026-11-05 [5d]
(Installed) docker-rootless-extras 29.9.3-1 UP TO DATE {44} 2026-11-04 [7d]
(Installed) zen-browser-bin 1.24.1b-1 UP TO DATE {343} 2026-11-06 [4d]
```

```
> taurus install
Getting installed packages
Updating vesktop-bin
Package vesktop-bin is up to date
Updating docker-rootless-extras
Package docker-rootless-extras is up to date
Updating zen-browser-bin
Package zen-browser-bin is up to date
```
