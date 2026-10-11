# How it works

## miIO over the LAN

Xiaomi Wi-Fi devices listen on UDP port 54321.

**Packets.** Each packet is a 32-byte header followed by an AES-128-CBC encrypted JSON-RPC
payload:

```
0x2131 | length (u16) | 0 (u32) | device id (u32) | stamp (u32) | md5 checksum (16 bytes)
key      = md5(token)
iv       = md5(key + token)
checksum = md5(first 16 header bytes + token + encrypted payload)
```

**Handshake.** A "hello" is a header of `0xff` bytes, sent unencrypted. The device replies
with its id and current stamp, and every request must carry a stamp ahead of the device's.
On current firmware the hello reply hides the token (all `ff`).

**Discovery.** This AC doesn't answer broadcast hellos, so finding it means probing every
address in the subnet. `aircon` does this automatically when the configured address stops
answering.

The client is in `crates/aircon/src/miio.rs`.

## The token

The token is created when the AC is paired, and only the AC and Xiaomi's cloud know it.
`aircon setup` reads it once from the account's device list, using the same calls as the Mi
Home app (`crates/aircon/src/cloud.rs`):

1. `GET account.xiaomi.com/longPolling/loginUrl` returns a QR image and a long-poll URL.
2. Long-polling until the QR is scanned in Mi Home returns `ssecurity`, `userId` and a
   `location`.
3. Following `location` sets the `serviceToken` cookie.
4. `POST <region>.api.io.mi.com/app/home/device_list`, RC4-encrypted:
   - the key is `sha256(ssecurity + nonce)`
   - the first 1024 keystream bytes are dropped
   - the request is signed with SHA-1

   It returns each device's `localip` and `token`.

Every region (`sg`, `cn`, `de`, `us`, `i2`, `ru`, `tw`, `in`) is searched, because accounts
live in one region but devices can appear in several.

## Properties

The AC speaks MIoT: `get_properties` and `set_properties` address values by service id
(siid) and property id (piid). The spec for this model is published at
[miot-spec.org](https://miot-spec.org/miot-spec-v2/instance?type=urn:miot-spec-v2:device:air-conditioner:0000A004:xiaomi-ra1r00:2).
The firmware accepts at most about 12 properties per request.

`crates/aircon/src/device.rs` maps the app's feature names onto the spec:

| Control | siid-piid | Spec name |
|---|---|---|
| `power` | 2-1 | `on` |
| `mode` | 2-2 | `mode` (2 cool, 3 dry, 4 fan) |
| `target_temperature` | 2-4 | `target-temperature` |
| `ai_energy_saving` | 2-7 | `eco` |
| `dryer` | 2-10 | `dryer` |
| `sleep` | 2-11 | `sleep-mode` |
| `gentle` | 2-18 | `list-soft-wind` |
| `turbo` | 2-29 | `max-level` |
| `low_watt` | 2-30 | `low-watt-level` (0 off, 1 = lowest power … 6) |
| `fan_speed` | 3-2 | `fan-level` (0 auto, 1–7) |
| `horizontal` | 3-3 + 3-26 | `horizontal-swing` + `horizontal-position` (1–5, left to right) |
| `vertical` | 3-4 + 3-25 | `vertical-swing` + `vertical-position` (1–5, up to down) |
| `airflow` | 3-11 | `wind-direction` (1 upward, 2 downward, 3 circulate) |
| `beep` | 5-1 | `alarm` |
| `display`, `display_brightness` | 6-1, 6-2 | indicator light |
| `room_temperature` | 4-7 | `temperature` |
| `outdoor_temperature` | 12-7 | `outdoor-temp` |
| `compressor_frequency` | 12-11 | `compressor-frequency` |
| `energy` | 20-1 | `power-consumption` (kWh) |
| `fault` | 13-1 | `fault-value` (positive = active, negative = cleared) |

Two of these are inferred from the names alone and haven't been confirmed against the app:
- **`ai_energy_saving` = `eco`.**
- **The louvre directions.** The spec's own labels for positions 1–5 are machine translations.

## Interlocks

The spec doesn't describe how the controls interact. These were found by writing each control
on a live unit and reading every related property back:

- **Turbo:** forces fan speed 7. Turning it off sets the fan to auto, and choosing a fan speed
  turns Turbo off.
- **Fan speed:** set by level. `fan-percent` (10-1, 0–101) follows the level (101 = auto) and
  ignores most direct writes.
- **Airflow patterns drive the louvres:**
  - upward and downward clear the vertical louvre setting, and circulate clears both axes
  - any manual louvre setting (swing or a direction, on either axis) ends the pattern
  - Gentle ends it too
- **Louvres:**
  - choosing a direction stops that axis swinging, and turning swing on clears the direction
  - while swinging, the horizontal position reads 9, which the spec doesn't list

The Omarchy panel mirrors these rules (`predict()` in `omarchy/Panel.qml`), so it updates at
once instead of flickering until the next read.
