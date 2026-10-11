//! acctl - control a Mijia air conditioner over the LAN, and serve it over MCP.

mod cloud;
mod config;
mod device;
mod mcp;
mod miio;

use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Map, Value};

use crate::device::{AirConditioner, CONTROLS};

#[derive(Parser)]
#[command(
    name = "acctl",
    about = "Control the Mijia air conditioner over the LAN"
)]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Show every control and reading
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Switch the AC on
    On,
    /// Switch the AC off
    Off,
    /// Set the target temperature: absolute (24) or relative (+0.5, -1)
    Temp {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Set the mode: cool, dry or fan
    Mode { value: String },
    /// Set the fan speed: auto or 1-7
    Fan { value: String },
    /// Set controls: acctl set turbo=on airflow=circulate horizontal=left-center
    Set {
        #[arg(required = true, value_name = "NAME=VALUE")]
        assignments: Vec<String>,
    },
    /// List every named control and the values it accepts
    Controls,
    /// Read or write any spec property: raw get 12 11, raw set 2 4 24.5
    Raw {
        #[command(subcommand)]
        action: Raw,
    },
    /// Save the AC's address and token: Xiaomi QR login, or --host/--token
    Setup {
        /// With several ACs on the account, pick one by name or device id
        #[arg(long)]
        device: Option<String>,
        /// Skip the cloud: the AC's IP address (needs --token)
        #[arg(long, requires = "token")]
        host: Option<String>,
        /// Skip the cloud: the AC's 32-character miIO token (needs --host)
        #[arg(long, requires = "host")]
        token: Option<String>,
    },
    /// Serve the AC as an MCP server on stdio
    Mcp,
}

#[derive(Subcommand)]
enum Raw {
    Get { siid: u32, piid: u32 },
    Set { siid: u32, piid: u32, value: String },
}

fn show(status: &Map<String, Value>) {
    let width = status.keys().map(String::len).max().unwrap_or(0);
    for (name, value) in status {
        let unit = CONTROLS
            .iter()
            .find(|s| s.name == name)
            .map_or("", |s| s.unit);
        let text = match value {
            Value::Bool(true) => "on".to_string(),
            Value::Bool(false) => "off".to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        println!("{name:<width$}  {text}{unit}");
    }
}

/// Show the login QR code in an image viewer; the caller kills it once scanned.
fn show_image(path: &std::path::Path) -> Option<Child> {
    ["imv", "feh", "eog", "xdg-open"].iter().find_map(|viewer| {
        Command::new(viewer)
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    })
}

/// Fetch the token from the Xiaomi cloud: QR login, then the account's device list.
fn setup_from_cloud(device: Option<&str>) -> Result<config::DeviceConfig> {
    let mut cloud = cloud::Cloud::new();
    let login = cloud.start_qr_login()?;
    let image = std::env::temp_dir().join(format!("xiaomi-login-{}.png", std::process::id()));
    std::fs::write(&image, cloud.download(&login.image_url)?)?;
    let viewer = show_image(&image);
    println!(
        "Scan the QR code in {} with the Mi Home app (Profile > scan icon).",
        image.display()
    );
    println!(
        "No viewer? Open this link on the phone signed in to Mi Home:\n  {}",
        login.login_url
    );

    let scanned = cloud.wait_for_scan(&login);
    if let Some(mut viewer) = viewer {
        let _ = viewer.kill();
    }
    let _ = std::fs::remove_file(&image);
    scanned?;

    let found = cloud.air_conditioners();
    let text = |d: &Value, key: &str| d[key].as_str().unwrap_or_default().to_string();
    let chosen: Vec<&Value> = match device {
        Some(wanted) => found
            .iter()
            .filter(|d| text(d, "name") == wanted || text(d, "did") == wanted)
            .collect(),
        None => found.iter().collect(),
    };
    match chosen.as_slice() {
        [one] => Ok(config::DeviceConfig {
            host: text(one, "localip"),
            token: text(one, "token"),
            did: text(one, "did"),
            model: text(one, "model"),
            name: text(one, "name"),
            mac: text(one, "mac"),
        }),
        [] if found.is_empty() => bail!("no air conditioner found on this Xiaomi account"),
        [] => bail!(
            "no air conditioner matches {device:?}; found: {}",
            describe(&found)
        ),
        _ => bail!(
            "several air conditioners found, pick one with --device: {}",
            describe(&found)
        ),
    }
}

fn describe(devices: &[Value]) -> String {
    let names: Vec<String> = devices
        .iter()
        .map(|d| {
            format!(
                "{} (did {}, {})",
                d["name"].as_str().unwrap_or("?"),
                d["did"].as_str().unwrap_or("?"),
                d["model"].as_str().unwrap_or("?")
            )
        })
        .collect();
    names.join(", ")
}

/// Use a token you already have (from another extractor, or a rooted phone).
fn setup_from_token(host: &str, token: &str) -> Result<config::DeviceConfig> {
    let mut device = miio::Device::new(host, token)?;
    let info = device.info().context("the AC did not accept that token")?;
    let model = info["model"].as_str().unwrap_or_default().to_string();
    Ok(config::DeviceConfig {
        host: host.to_string(),
        token: token.to_string(),
        did: device.device_id().to_string(),
        name: model.clone(),
        model,
        mac: info["mac"].as_str().unwrap_or_default().to_string(),
    })
}

fn setup(device: Option<&str>, host: Option<&str>, token: Option<&str>) -> Result<()> {
    let config = match (host, token) {
        (Some(host), Some(token)) => setup_from_token(host, token)?,
        _ => setup_from_cloud(device)?,
    };
    config::save(&config)?;
    println!(
        "Saved {} ({}, {}) to {}",
        config.name,
        config.model,
        config.host,
        config::path().display()
    );
    if !device::TESTED_MODELS.contains(&config.model.as_str()) {
        eprintln!(
            "warning: {} has not been tested; controls are mapped for {}. Check `acctl status` before relying on them.",
            config.model,
            device::TESTED_MODELS.join(", ")
        );
    }
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    let set = |name: &str, value: Value| -> Result<()> {
        show(&AirConditioner::open()?.set(&[(name.to_string(), value)])?);
        Ok(())
    };
    match cli.action {
        Action::Setup {
            device,
            host,
            token,
        } => setup(device.as_deref(), host.as_deref(), token.as_deref()),
        Action::Mcp => mcp::serve(),
        Action::Controls => {
            for spec in CONTROLS {
                let access = if spec.writable { "rw" } else { "r " };
                println!(
                    "{access}  {:<20} {} [{}]",
                    spec.name,
                    spec.description,
                    spec.choices()
                );
            }
            Ok(())
        }
        Action::Status { json } => {
            let status = AirConditioner::open()?.status(&[])?;
            if json {
                println!("{}", Value::Object(status));
            } else {
                show(&status);
            }
            Ok(())
        }
        Action::On => set("power", Value::Bool(true)),
        Action::Off => set("power", Value::Bool(false)),
        Action::Mode { value } => set("mode", Value::from(value)),
        Action::Fan { value } => set("fan_speed", Value::from(value)),
        Action::Temp { value } => {
            let mut ac = AirConditioner::open()?;
            let target = if value.starts_with(['+', '-']) {
                let delta: f64 = value.parse().context("not a number")?;
                let current = ac.status(&["target_temperature"])?["target_temperature"]
                    .as_f64()
                    .unwrap_or(0.0);
                current + delta
            } else {
                value.parse().context("not a number")?
            };
            show(&ac.set(&[("target_temperature".into(), Value::from(target))])?);
            Ok(())
        }
        Action::Set { assignments } => {
            let mut values = Vec::new();
            for assignment in assignments {
                let Some((name, value)) = assignment.split_once('=') else {
                    bail!("expected NAME=VALUE, got {assignment:?}");
                };
                values.push((name.to_string(), Value::from(value)));
            }
            show(&AirConditioner::open()?.set(&values)?);
            Ok(())
        }
        Action::Raw { action } => {
            let mut ac = AirConditioner::open()?;
            let reply = match action {
                Raw::Get { siid, piid } => ac.raw_get(siid, piid)?,
                Raw::Set { siid, piid, value } => {
                    let value = serde_json::from_str(&value)
                        .context("value must be JSON, e.g. 24.5, true or \"text\"")?;
                    ac.raw_set(siid, piid, value)?
                }
            };
            println!("{reply}");
            Ok(())
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("acctl: {error:#}");
        std::process::exit(1);
    }
}
