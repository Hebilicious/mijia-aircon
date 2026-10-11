# Agent guide

Local control for a Xiaomi Mijia air conditioner: a Rust CLI and MCP server (`acctl`), and an
Omarchy shell widget. Read [README.md](README.md) for usage and
[docs/protocol.md](docs/protocol.md) for the protocol and the AC's interlocks.

## Layout

| Path | What |
|---|---|
| `crates/acctl/src/miio.rs` | miIO LAN client: packets, handshake, subnet rediscovery |
| `crates/acctl/src/device.rs` | `CONTROLS` table (names → spec properties), `AirConditioner` |
| `crates/acctl/src/cloud.rs` | Xiaomi QR login and device list, used only by `acctl setup` |
| `crates/acctl/src/mcp.rs` | MCP server on stdio (rmcp) |
| `crates/acctl/src/main.rs` | CLI (clap) |
| `omarchy/` | Quickshell bar widget for Omarchy (`Panel.qml`, `manifest.json`, `install.sh`) |

## Toolchain

proto pins moon and Rust in `.prototools`, and moon runs the tasks. Don't install Rust or moon
any other way.

```sh
moon run acctl:check acctl:lint acctl:test   # must pass before committing
moon run acctl:fmt                           # rustfmt
moon run omarchy:check
moon run acctl:install                       # ~/.local/bin/acctl
moon run omarchy:install                     # widget; restarts the Omarchy shell
```

## Rules

- **A live AC is a shared, physical device.** Before experimenting, record `acctl status
  --json`. Afterwards, restore every value you changed and confirm with a second status read.
- **Never cycle power in tests.** The compressor has restart protection.
- **No personal data in the repo:**
  - tokens or `~/.config/acctl/config.json`
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
