//! The Mijia Air Conditioner Pro Eco (xiaomi.airc.ra1r00), by name.
//!
//! Property ids come from the published MIoT spec:
//! https://miot-spec.org/miot-spec-v2/instance?type=urn:miot-spec-v2:device:air-conditioner:0000A004:xiaomi-ra1r00:2
//!
//! Names follow the Mi Home app rather than the spec's machine translations
//! (the spec's "max-level" is the app's Turbo). Each control maps a name onto
//! one or more (siid, piid) properties; anything not listed can still be
//! reached with `raw_get` / `raw_set`.
//!
//! Behaviour observed on the unit (firmware 1.0.6), which the UI relies on:
//! - Turbo forces fan speed 7; switching Turbo off returns the fan to auto,
//!   and picking a fan speed switches Turbo off.
//! - Fan speed is set by level; the spec's 0-101 "fan-percent" (10-1) follows
//!   the level (101 = auto) and ignores most direct writes.
//! - Airflow patterns drive the louvres: upward/downward clear the vertical
//!   louvre setting, circulate clears both. Any manual louvre setting (swing
//!   or direction, either axis) ends the airflow pattern, and so does Gentle.
//! - Picking a louvre direction stops that louvre's swing; turning swing on
//!   clears the direction. While swinging, the horizontal position reads 9.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::config::{self, DeviceConfig};
use crate::miio::{self, Device, NoReply};

pub enum Kind {
    Bool,
    Float { min: f64, max: f64, step: f64 },
    Int,
    Enum(&'static [(i64, &'static str)]),
    Fault,
}

pub enum Control {
    Prop {
        siid: u32,
        piid: u32,
        kind: Kind,
    },
    /// A louvre axis: a swing switch plus five fixed positions, read and set as
    /// one choice: "swing", "off", or one of `positions` (spec values 1-5).
    Louvre {
        swing: (u32, u32),
        position: (u32, u32),
        positions: [&'static str; 5],
    },
}

pub struct Spec {
    pub name: &'static str,
    pub description: &'static str,
    pub writable: bool,
    pub unit: &'static str,
    pub control: Control,
}

const MODES: &[(i64, &str)] = &[(2, "cool"), (3, "dry"), (4, "fan")];
const FAN_SPEEDS: &[(i64, &str)] = &[
    (0, "auto"),
    (1, "1"),
    (2, "2"),
    (3, "3"),
    (4, "4"),
    (5, "5"),
    (6, "6"),
    (7, "7"),
];
const LOW_WATT: &[(i64, &str)] = &[
    (0, "off"),
    (1, "1"),
    (2, "2"),
    (3, "3"),
    (4, "4"),
    (5, "5"),
    (6, "6"),
];
const ON_OFF: &[(i64, &str)] = &[(0, "off"), (1, "on")];
const AIRFLOWS: &[(i64, &str)] = &[(0, "off"), (1, "upward"), (2, "downward"), (3, "circulate")];
const BRIGHTNESS: &[(i64, &str)] = &[(0, "auto"), (1, "medium"), (2, "high")];

/// Indoor/outdoor unit fault codes as shown on the AC's display. The spec
/// reports a fault as a positive value and its clearing as the negated value.
const FAULTS: &[(i64, &str)] = &[
    (4, "F2.4"),
    (5, "F3.2"),
    (6, "P1"),
    (7, "P2.1"),
    (8, "P2.2"),
    (9, "P2.3"),
    (10, "P2.4"),
    (11, "P2.5"),
    (12, "P4"),
    (13, "P5"),
    (14, "P6"),
    (15, "P8"),
    (16, "PA"),
    (17, "PC"),
    (18, "U2"),
    (19, "U3"),
    (20, "U4"),
    (21, "U5"),
    (22, "U6.1"),
    (23, "U6.2"),
    (24, "U8"),
    (25, "U0"),
    (26, "C1"),
    (27, "C2"),
    (28, "C3"),
    (29, "C4"),
    (30, "C5"),
    (31, "U2.1"),
    (32, "E2.1"),
    (33, "E2.6"),
    (34, "E2.5"),
    (35, "E2.2"),
    (36, "F2.6"),
    (37, "F7"),
    (47, "F5"),
];

const fn prop(siid: u32, piid: u32, kind: Kind) -> Control {
    Control::Prop { siid, piid, kind }
}

const fn rw(name: &'static str, description: &'static str, control: Control) -> Spec {
    Spec {
        name,
        description,
        writable: true,
        unit: "",
        control,
    }
}

const fn sensor(
    name: &'static str,
    description: &'static str,
    unit: &'static str,
    control: Control,
) -> Spec {
    Spec {
        name,
        description,
        writable: false,
        unit,
        control,
    }
}

pub static CONTROLS: &[Spec] = &[
    // core
    rw("power", "AC on/off", prop(2, 1, Kind::Bool)),
    rw("mode", "Operating mode", prop(2, 2, Kind::Enum(MODES))),
    Spec {
        name: "target_temperature",
        description: "Target temperature, 0.5° steps",
        writable: true,
        unit: "°C",
        control: prop(
            2,
            4,
            Kind::Float {
                min: 16.0,
                max: 31.0,
                step: 0.5,
            },
        ),
    },
    rw(
        "fan_speed",
        "Fan speed: auto or 1 (quiet) to 7",
        prop(3, 2, Kind::Enum(FAN_SPEEDS)),
    ),
    // comfort
    rw(
        "ai_energy_saving",
        "AI energy saving",
        prop(2, 7, Kind::Bool),
    ),
    rw(
        "turbo",
        "Turbo: maximum cooling, fan at 7",
        prop(2, 29, Kind::Bool),
    ),
    rw(
        "low_watt",
        "Low-watt power cap: off or level 1 (lowest power) to 6",
        prop(2, 30, Kind::Enum(LOW_WATT)),
    ),
    rw(
        "gentle",
        "Gentle (draught-free) wind",
        prop(2, 18, Kind::Enum(ON_OFF)),
    ),
    rw("sleep", "Sleep mode", prop(2, 11, Kind::Bool)),
    rw(
        "dryer",
        "Self-drying of the coil after switching off",
        prop(2, 10, Kind::Bool),
    ),
    // airflow
    rw(
        "airflow",
        "Airflow pattern",
        prop(3, 11, Kind::Enum(AIRFLOWS)),
    ),
    rw(
        "horizontal",
        "Horizontal louvres: swing, off, or a fixed direction",
        Control::Louvre {
            swing: (3, 3),
            position: (3, 26),
            positions: ["left", "left-center", "center", "right-center", "right"],
        },
    ),
    rw(
        "vertical",
        "Vertical louvres: swing, off, or a fixed direction",
        Control::Louvre {
            swing: (3, 4),
            position: (3, 25),
            positions: ["up", "up-center", "center", "down-center", "down"],
        },
    ),
    // unit
    rw(
        "display",
        "Indoor unit display light",
        prop(6, 1, Kind::Bool),
    ),
    rw(
        "display_brightness",
        "Display brightness",
        prop(6, 2, Kind::Enum(BRIGHTNESS)),
    ),
    rw("beep", "Beep on commands", prop(5, 1, Kind::Bool)),
    // sensors
    sensor(
        "room_temperature",
        "Indoor temperature",
        "°C",
        prop(
            4,
            7,
            Kind::Float {
                min: -50.0,
                max: 150.0,
                step: 0.1,
            },
        ),
    ),
    sensor(
        "outdoor_temperature",
        "Outdoor temperature",
        "°C",
        prop(
            12,
            7,
            Kind::Float {
                min: -100.0,
                max: 150.0,
                step: 0.1,
            },
        ),
    ),
    sensor(
        "energy",
        "Lifetime energy use",
        "kWh",
        prop(
            20,
            1,
            Kind::Float {
                min: 0.0,
                max: 999_999.99,
                step: 0.01,
            },
        ),
    ),
    sensor(
        "compressor_frequency",
        "Compressor frequency",
        "Hz",
        prop(
            12,
            11,
            Kind::Float {
                min: 0.0,
                max: 200.0,
                step: 1.0,
            },
        ),
    ),
    sensor(
        "indoor_fan_rpm",
        "Indoor fan speed",
        "rpm",
        prop(12, 3, Kind::Int),
    ),
    sensor(
        "fault",
        "Fault code shown on the unit",
        "",
        prop(13, 1, Kind::Fault),
    ),
];

/// Models the control table was verified against on real hardware. Sibling
/// `xiaomi.airc.*` models publish similar specs but may number things differently.
pub const TESTED_MODELS: &[&str] = &["xiaomi.airc.ra1r00"];

pub fn spec(name: &str) -> Result<&'static Spec> {
    CONTROLS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow!("unknown control {name:?}"))
}

/// Text form of a JSON value, so CLI strings and MCP JSON parse the same way.
fn as_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Bool(b) => (if *b { "on" } else { "off" }).to_string(),
        other => other.to_string(),
    }
}

impl Spec {
    pub fn reads(&self) -> Vec<(u32, u32)> {
        match &self.control {
            Control::Prop { siid, piid, .. } => vec![(*siid, *piid)],
            Control::Louvre {
                swing, position, ..
            } => vec![*swing, *position],
        }
    }

    pub fn choices(&self) -> String {
        match &self.control {
            Control::Prop {
                kind: Kind::Bool, ..
            } => "on, off".into(),
            Control::Prop {
                kind: Kind::Enum(table),
                ..
            } => table
                .iter()
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", "),
            Control::Prop {
                kind: Kind::Float { min, max, .. },
                ..
            } => format!("{min}-{max}{}", self.unit),
            Control::Prop {
                kind: Kind::Int, ..
            } => self.unit.into(),
            Control::Prop {
                kind: Kind::Fault, ..
            } => "display code".into(),
            Control::Louvre { positions, .. } => format!("swing, off, {}", positions.join(", ")),
        }
    }

    fn decode(&self, values: &[Value]) -> Value {
        match &self.control {
            Control::Prop {
                kind: Kind::Enum(table),
                ..
            } => {
                let code = values[0].as_i64();
                match table.iter().find(|(c, _)| Some(*c) == code) {
                    Some((_, name)) => Value::from(*name),
                    None => values[0].clone(),
                }
            }
            Control::Prop {
                kind: Kind::Fault, ..
            } => {
                let code = values[0].as_i64().unwrap_or(0);
                let name = FAULTS
                    .iter()
                    .find(|(c, _)| *c == code.abs())
                    .map(|(_, n)| *n);
                match (code, name) {
                    (0, _) => Value::from("none"),
                    (c, Some(name)) if c < 0 => Value::from(format!("{name} cleared")),
                    (_, Some(name)) => Value::from(name),
                    _ => values[0].clone(),
                }
            }
            Control::Prop { .. } => values[0].clone(),
            Control::Louvre { positions, .. } => {
                let swinging = values[0].as_bool().unwrap_or(false);
                match values[1].as_i64() {
                    Some(p @ 1..=5) => Value::from(positions[p as usize - 1]),
                    _ if swinging => Value::from("swing"),
                    _ => Value::from("off"),
                }
            }
        }
    }

    fn encode(&self, value: &Value) -> Result<Vec<(u32, u32, Value)>> {
        let text = as_text(value);
        match &self.control {
            Control::Prop { siid, piid, kind } => {
                let device_value = match kind {
                    Kind::Bool => match text.as_str() {
                        "on" | "true" | "1" | "yes" => Value::Bool(true),
                        "off" | "false" | "0" | "no" => Value::Bool(false),
                        _ => bail!("{}: expected on/off, got {text:?}", self.name),
                    },
                    Kind::Enum(table) => match table.iter().find(|(_, name)| *name == text) {
                        Some((code, _)) => Value::from(*code),
                        None => bail!(
                            "{}: expected one of {}, got {text:?}",
                            self.name,
                            self.choices()
                        ),
                    },
                    Kind::Float { min, max, step } => {
                        let number: f64 = text
                            .parse()
                            .map_err(|_| anyhow!("{}: not a number: {text:?}", self.name))?;
                        if !(*min..=*max).contains(&number) {
                            bail!(
                                "{}: expected {min}..{max}{}, got {text}",
                                self.name,
                                self.unit
                            );
                        }
                        Value::from((number / step).round() * step)
                    }
                    Kind::Int => Value::from(
                        text.parse::<i64>()
                            .map_err(|_| anyhow!("{}: not an integer", self.name))?,
                    ),
                    Kind::Fault => bail!("{} is read-only", self.name),
                };
                Ok(vec![(*siid, *piid, device_value)])
            }
            Control::Louvre {
                swing,
                position,
                positions,
            } => match text.as_str() {
                "swing" => Ok(vec![(swing.0, swing.1, Value::Bool(true))]),
                "off" => Ok(vec![
                    (position.0, position.1, Value::from(0)),
                    (swing.0, swing.1, Value::Bool(false)),
                ]),
                _ => match positions.iter().position(|p| *p == text) {
                    Some(i) => Ok(vec![(position.0, position.1, Value::from(i + 1))]),
                    None => bail!(
                        "{}: expected one of {}, got {text:?}",
                        self.name,
                        self.choices()
                    ),
                },
            },
        }
    }
}

pub struct AirConditioner {
    device: Device,
    config: DeviceConfig,
}

impl AirConditioner {
    pub fn open() -> Result<Self> {
        let config = config::load()?;
        if config.host.is_empty() || config.token.is_empty() {
            bail!(
                "{} has no host or token; run `aircon setup`",
                config::path().display()
            );
        }
        Ok(Self {
            device: Device::new(&config.host, &config.token)?,
            config,
        })
    }

    /// Run a device call; if the AC went quiet, look for it at a new IP once.
    fn call<T>(&mut self, f: impl Fn(&mut Device) -> Result<T>) -> Result<T> {
        match f(&mut self.device) {
            Err(error) if error.is::<NoReply>() => {
                let Ok(did) = self.config.did.parse::<u32>() else {
                    return Err(error);
                };
                match miio::discover(did, &self.device.host, Duration::from_secs(2)) {
                    Some(host) if host != self.device.host => {
                        self.device.host = host.clone();
                        self.config.host = host;
                        config::save(&self.config)?;
                        f(&mut self.device)
                    }
                    _ => Err(error),
                }
            }
            result => result,
        }
    }

    /// Read named controls (all of them when `names` is empty), in table order.
    pub fn status(&mut self, names: &[&str]) -> Result<Map<String, Value>> {
        let specs: Vec<&Spec> = if names.is_empty() {
            CONTROLS.iter().collect()
        } else {
            names.iter().map(|n| spec(n)).collect::<Result<_>>()?
        };
        let mut wanted: Vec<(u32, u32)> = Vec::new();
        for reference in specs.iter().flat_map(|s| s.reads()) {
            if !wanted.contains(&reference) {
                wanted.push(reference);
            }
        }

        let mut values: HashMap<(u32, u32), Value> = HashMap::new();
        // The firmware caps how many properties fit in one request.
        for chunk in wanted.chunks(12) {
            let replies = self.call(|d| d.get_properties(chunk))?;
            for (reference, reply) in chunk.iter().zip(replies) {
                if reply["code"] == 0 {
                    values.insert(*reference, reply["value"].clone());
                }
            }
        }

        let mut status = Map::new();
        for spec in specs {
            let read: Option<Vec<Value>> = spec
                .reads()
                .iter()
                .map(|r| values.get(r).cloned())
                .collect();
            if let Some(read) = read {
                status.insert(spec.name.to_string(), spec.decode(&read));
            }
        }
        Ok(status)
    }

    /// Set several named controls at once; returns their values read back.
    pub fn set(&mut self, values: &[(String, Value)]) -> Result<Map<String, Value>> {
        let mut writes = Vec::new();
        for (name, value) in values {
            let spec = spec(name)?;
            if !spec.writable {
                bail!("{name} is read-only");
            }
            writes.extend(spec.encode(value)?);
        }
        let replies = self.call(|d| d.set_properties(&writes))?;
        let rejected: Vec<String> = writes
            .iter()
            .zip(&replies)
            .filter(|(_, reply)| reply["code"] != 0)
            .map(|((s, p, _), reply)| format!("{s}-{p} (code {})", reply["code"]))
            .collect();
        if !rejected.is_empty() {
            bail!("the AC rejected: {}", rejected.join(", "));
        }
        let names: Vec<&str> = values.iter().map(|(name, _)| name.as_str()).collect();
        self.status(&names)
    }

    pub fn raw_get(&mut self, siid: u32, piid: u32) -> Result<Value> {
        Ok(self.call(|d| d.get_properties(&[(siid, piid)]))?.remove(0))
    }

    pub fn raw_set(&mut self, siid: u32, piid: u32, value: Value) -> Result<Value> {
        Ok(self
            .call(|d| d.set_properties(&[(siid, piid, value.clone())]))?
            .remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encode(name: &str, value: Value) -> Result<Vec<(u32, u32, Value)>> {
        spec(name)?.encode(&value)
    }

    #[test]
    fn switches_accept_words_and_json_booleans() {
        assert_eq!(
            encode("turbo", json!("on")).unwrap(),
            vec![(2, 29, json!(true))]
        );
        assert_eq!(
            encode("turbo", json!(false)).unwrap(),
            vec![(2, 29, json!(false))]
        );
        assert!(encode("turbo", json!("maybe")).is_err());
    }

    #[test]
    fn enums_map_names_to_spec_codes() {
        assert_eq!(
            encode("mode", json!("dry")).unwrap(),
            vec![(2, 2, json!(3))]
        );
        assert_eq!(
            encode("fan_speed", json!("auto")).unwrap(),
            vec![(3, 2, json!(0))]
        );
        assert_eq!(
            encode("airflow", json!("Circulate")).unwrap(),
            vec![(3, 11, json!(3))]
        );
        assert!(encode("mode", json!("heat")).is_err());
        assert_eq!(spec("low_watt").unwrap().decode(&[json!(4)]), json!("4"));
    }

    #[test]
    fn temperature_is_range_checked_and_snapped_to_half_degrees() {
        assert_eq!(
            encode("target_temperature", json!(24.3)).unwrap(),
            vec![(2, 4, json!(24.5))]
        );
        assert!(encode("target_temperature", json!(40)).is_err());
    }

    #[test]
    fn louvres_combine_swing_and_position() {
        let horizontal = spec("horizontal").unwrap();
        assert_eq!(
            horizontal.encode(&json!("swing")).unwrap(),
            vec![(3, 3, json!(true))]
        );
        assert_eq!(
            horizontal.encode(&json!("left-center")).unwrap(),
            vec![(3, 26, json!(2))]
        );
        assert_eq!(
            horizontal.encode(&json!("off")).unwrap(),
            vec![(3, 26, json!(0)), (3, 3, json!(false))]
        );
        assert!(horizontal.encode(&json!("up")).is_err());

        // While swinging the unit reports position 9, which is not a direction.
        assert_eq!(horizontal.decode(&[json!(true), json!(9)]), json!("swing"));
        assert_eq!(horizontal.decode(&[json!(true), json!(3)]), json!("center"));
        assert_eq!(
            spec("vertical").unwrap().decode(&[json!(false), json!(5)]),
            json!("down")
        );
        assert_eq!(horizontal.decode(&[json!(false), json!(0)]), json!("off"));
    }

    #[test]
    fn faults_decode_to_display_codes() {
        let fault = spec("fault").unwrap();
        assert_eq!(fault.decode(&[json!(0)]), json!("none"));
        assert_eq!(fault.decode(&[json!(4)]), json!("F2.4"));
        assert_eq!(fault.decode(&[json!(-26)]), json!("C1 cleared"));
        assert!(encode("fault", json!("none")).is_err());
    }

    #[test]
    fn every_control_name_is_unique() {
        for (i, a) in CONTROLS.iter().enumerate() {
            assert!(
                CONTROLS[i + 1..].iter().all(|b| b.name != a.name),
                "duplicate control {}",
                a.name
            );
        }
    }
}
