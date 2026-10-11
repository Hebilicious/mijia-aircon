# xiaomi-aircon

Control a Xiaomi **Mijia Air Conditioner Pro Eco** (`xiaomi.airc.ra1r00`) from your computer,
over your local network, without the Mi Home app.

- **`aircon`**: a CLI for every feature the app has (power, mode, temperature, fan speed, AI
  energy saving, Turbo, low watt, Gentle, Sleep, airflow, louvre swing and direction,
  display) plus readings (room and outdoor temperature, energy use, compressor speed, fault code).
- **`aircon mcp`**: the same controls as an [MCP](https://modelcontextprotocol.io) server, so
  Claude and other agents can run the AC.
- **Omarchy bar widget**: a panel next to the battery in the [Omarchy](https://omarchy.org)
  shell, with sliders, toggles and direction buttons.

Control is local (miIO over UDP). Xiaomi's cloud is used once, to fetch the AC's token.

## Install

### Prebuilt release

Each [release](https://github.com/Hebilicious/xiaomi-aircon/releases/latest) has a static
binary for Linux x86_64 and one for macOS on Apple silicon, plus the Omarchy widget.

```sh
mkdir -p ~/.local/bin
curl -fsSL https://github.com/Hebilicious/xiaomi-aircon/releases/latest/download/aircon-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin aircon
```

On a Mac, use `aircon-aarch64-apple-darwin.tar.gz`. Make sure `~/.local/bin` is on your `PATH`.

The widget (Omarchy only):

```sh
curl -fsSL https://github.com/Hebilicious/xiaomi-aircon/releases/latest/download/xiaomi-aircon-omarchy.tar.gz | tar -xz
bash xiaomi-aircon-omarchy/install.sh
```

Each release also has a `SHA256SUMS` file for checking the downloads.

### From source

[proto](https://moonrepo.dev/proto) installs the toolchain pinned in `.prototools` (Rust and
[moon](https://moonrepo.dev/moon)), so you don't need Rust already installed.

```sh
git clone https://github.com/Hebilicious/xiaomi-aircon && cd xiaomi-aircon
proto install                  # installs the pinned Rust and moon
moon run aircon:install        # builds and installs ~/.local/bin/aircon
moon run omarchy:install       # optional: the Omarchy widget (also installs aircon)
```

## Set up

The AC only accepts commands encrypted with its 16-byte token. It doesn't reveal the token on
the network, and only the AC and Xiaomi's cloud know it. Fetch it once:

```sh
aircon setup
```

A QR code opens. Scan it with the Mi Home app (Profile > scan icon), using the account the AC
is paired to. `aircon` reads the AC's address and token from your device list and saves them
to `~/.config/aircon/config.json` (mode 600). The cloud session isn't kept.

- **Several ACs on the account:** `aircon setup --device "Bedroom AC"` (a name or device id).
- **Already have the token** (from another extractor): `aircon setup --host 192.168.1.50 --token <32 hex chars>`.

Keep the token private: anyone on your network who has it can control the AC. If the AC gets
a new IP address, `aircon` finds it again by device id and updates the config.

## Use

```sh
aircon status                   # everything, or --json
aircon on | off
aircon temp 24                  # or +0.5, -1
aircon mode cool                # cool | dry | fan
aircon fan auto                 # auto | 1..7
aircon set turbo=on low_watt=3 airflow=circulate
aircon set horizontal=swing vertical=down-center
aircon controls                 # every control and the values it accepts
aircon raw get 12 11            # any property from the spec, by siid/piid
```

| Control | Values |
|---|---|
| `power` | `on`, `off` |
| `mode` | `cool`, `dry`, `fan` (this unit has no heating) |
| `target_temperature` | 16–31 °C in 0.5° steps |
| `fan_speed` | `auto`, `1`–`7` |
| `ai_energy_saving`, `turbo`, `gentle`, `sleep`, `dryer` | `on`, `off` |
| `low_watt` | `off`, `1`–`6` (power cap; `1` draws the least) |
| `airflow` | `off`, `upward`, `downward`, `circulate` |
| `horizontal` | `swing`, `off`, `left`, `left-center`, `center`, `right-center`, `right` |
| `vertical` | `swing`, `off`, `up`, `up-center`, `center`, `down-center`, `down` |
| `display`, `beep` | `on`, `off` |
| `display_brightness` | `auto`, `medium`, `high` |

The AC links some of these itself:
- Turbo forces fan speed 7, and choosing a fan speed turns Turbo off.
- An airflow pattern takes over the louvres, and setting a louvre ends the pattern.

See [docs/protocol.md](docs/protocol.md) for the full list.

## MCP server

Claude Code:

```sh
claude mcp add aircon -s user -- ~/.local/bin/aircon mcp
```

Other clients: run `aircon mcp` over stdio, for example:

```json
{ "mcpServers": { "aircon": { "command": "/home/you/.local/bin/aircon", "args": ["mcp"] } } }
```

Tools: `get_status`, `set_power`, `set_temperature`, `set_mode`, `set_fan_speed`, and
`set_controls` (several controls at once; its description lists them all).

## Omarchy widget

`moon run omarchy:install` from a clone, or the release's `install.sh` (see [Install](#install)). Either way it:
- copies the plugin to `~/.config/omarchy/plugins/xiaomi.aircon`
- puts it on the bar before the battery
- restarts the shell

On the bar:
- **Button:** shows the target temperature while cooling.
- **Click:** opens the panel.
- **Right-click:** toggles power.
- **Scroll:** changes the target by 0.5°.

The panel has a power switch and the temperature, then mode, a fan-speed slider with Auto,
comfort toggles, a low-watt slider, airflow, louvres (swing or five direction arrows per
axis), readings, and a "More" section for the display, beep and self-dry. Controls that don't
apply are hidden: everything when the AC is off, and the compressor features in fan mode.

If `aircon` isn't on the shell's `PATH`, set its location in the bar entry in
`~/.config/omarchy/shell.json`: `{ "id": "xiaomi.aircon", "command": "/path/to/aircon" }`.

## Other models

The control table was verified on `xiaomi.airc.ra1r00` (firmware 1.0.6). Other Xiaomi ACs use
the same protocol but may number their properties differently. `aircon setup` warns when the
model is untested.

To check a different model, compare its spec on
[miot-spec.org](https://miot-spec.org/miot-spec-v2/instances?status=released) with the table
in `crates/aircon/src/device.rs`. Contributions for other models are welcome.

## Develop

```sh
moon run aircon:check aircon:lint aircon:test   # clippy, rustfmt, unit tests
moon run omarchy:check
```

[AGENTS.md](AGENTS.md) describes the layout and the rules for working on a live AC.

## Credits

- The miIO protocol was reverse engineered by the
  [python-miio](https://github.com/rytilahti/python-miio) project and others.
- The cloud login follows
  [Xiaomi-cloud-tokens-extractor](https://github.com/PiotrMachowski/Xiaomi-cloud-tokens-extractor).
- Property definitions come from Xiaomi's published [MIoT spec](https://miot-spec.org).

Not affiliated with Xiaomi. MIT licensed.
