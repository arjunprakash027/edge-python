# Edge Python CLI

The Edge Python developer CLI. Write `.py`, run it, serve it, ship it. You never compile anything: `edge` hosts the Edge Python runtime in a headless Chromium provisioned by `install.sh`, then runs your code against it.

```bash
edge run app.py     # run a script
edge serve          # dev server with live reload
edge repl           # interactive shell
edge test           # run your *_test.py files (not implemented yet)
edge init my-app    # scaffold a project
edge add network    # add a package to packages.json
edge remove network # remove a package
edge build          # bundle to dist/
edge uninstall      # remove the binary, PATH entry, optionally Chromium
```

## Install

```bash
# Prebuilt binary (recommended)
curl -fsSL https://cdn.edgepython.com/cli/install.sh | sh

# Or from source (any platform with Rust + Cargo)
cargo install --path cli
```

`install.sh` drops the binary at `~/.local/bin/edge`, puts it on `PATH`, and provisions Chromium via the host package manager if it is not already present. Re-run the same line to upgrade. Point `EDGE_CHROME_PATH=/path/to/chrome` at a custom browser.

Full command reference, flags, and examples: [edgepython.com/reference/cli](https://edgepython.com/reference/cli).

## License

MIT OR Apache-2.0
