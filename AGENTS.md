# Agent guide

Local control for a Xiaomi Mijia air conditioner: a Rust CLI and MCP server (`aircon`), and an
Omarchy shell widget. Read [README.md](README.md) for usage and
[docs/protocol.md](docs/protocol.md) for the protocol and the AC's interlocks.

## Layout

| Path | What |
|---|---|
| `crates/aircon/src/miio.rs` | miIO LAN client: packets, handshake, subnet rediscovery |
| `crates/aircon/src/device.rs` | `CONTROLS` table (names → spec properties), `AirConditioner` |
| `crates/aircon/src/cloud.rs` | Xiaomi QR login and device list, used only by `aircon setup` |
| `crates/aircon/src/mcp.rs` | MCP server on stdio (rmcp) |
| `crates/aircon/src/main.rs` | CLI (clap) |
| `omarchy/` | Quickshell bar widget for Omarchy (`Panel.qml`, `manifest.json`, `install.sh`) |

## Toolchain

proto pins moon and Rust in `.prototools`, and moon runs the tasks. Don't install Rust or moon
any other way.

```sh
proto install                                   # pinned Rust and moon
moon run aircon:check aircon:lint aircon:test   # must pass before committing
moon run aircon:fmt                             # rustfmt
moon run omarchy:check
moon run aircon:install                         # ~/.local/bin/aircon
moon run omarchy:install                        # widget; restarts the Omarchy shell
```

## Releases

Push a `vX.Y.Z` tag that matches `version` in `crates/aircon/Cargo.toml`. The release workflow
then builds the Linux (static musl) and macOS binaries, packages the Omarchy widget, and
publishes them with `SHA256SUMS` as a GitHub release. Asset names have no version in them, so
the README's `releases/latest/download/...` links keep working.

## Rules

- **A live AC is a shared, physical device.** Before experimenting, record `aircon status
  --json`. Afterwards, restore every value you changed and confirm with a second status read.
- **Never cycle power in tests.** The compressor has restart protection.
- **No personal data in the repo:**
  - tokens or `~/.config/aircon/config.json`
  - IP or MAC addresses
  - device or account ids
- **Adding a control:**
  1. Add a row to `CONTROLS`.
  2. Verify it on hardware by writing it and reading related properties back.
  3. Record any interlocks in `docs/protocol.md`, the module docs in `device.rs`, and
     `predict()` in `omarchy/Panel.qml`.
  4. Add a unit test for its encoding.
- **Errors:** keep them readable. CLI and MCP errors go straight to users and agents, so name
  the control and the accepted values.
- **Hot reload:** the Omarchy shell doesn't reload an already-loaded plugin's QML, so run
  `omarchy restart shell` after editing it (`install.sh` does this).
- **New models:** add the model to `TESTED_MODELS` only after checking every control on one.
