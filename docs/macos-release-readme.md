# Agentlog macOS release

This archive contains a native macOS build of Agentlog. It is not Developer ID
signed or notarized.

Install into a user-owned prefix:

```sh
./install.sh "$HOME/.local"
```

Add `$HOME/.local/bin` to `PATH` if needed. Running `./install.sh` without a
prefix installs into `/usr/local`, which may require elevated permissions. A
successful reinstall atomically replaces an existing regular Agentlog binary;
the installer refuses to replace a symlink or non-file entry.

Start with:

```sh
agentlog sync
agentlog browse
```

See [MVP scope and limitations](docs/mvp-scope.md) and
[MVP acceptance](docs/mvp-acceptance.md) for the shipped boundary and review
procedure.
