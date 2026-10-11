//! MCP server (stdio): `acctl mcp`. Register with
//!   claude mcp add aircon -s user -- ~/.local/bin/acctl mcp

use std::sync::{Arc, Mutex};

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::device::{AirConditioner, CONTROLS};

#[derive(Deserialize, schemars::JsonSchema)]
struct PowerArgs {
    /// true to switch on, false to switch off
    on: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct TemperatureArgs {
    /// Target in °C, 16-31 in 0.5 steps
    celsius: f64,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Cool,
    Dry,
    Fan,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ModeArgs {
    mode: Mode,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct FanArgs {
    /// "auto", or "1" (quietest) to "7" (strongest)
    speed: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ControlsArgs {
    /// Control name to value, e.g. {"turbo": true, "airflow": "circulate"}
    values: Map<String, Value>,
}

fn controls_description() -> String {
    let guide: Vec<String> = CONTROLS
        .iter()
        .filter(|s| s.writable)
        .map(|s| format!("- {}: {} ({})", s.name, s.description, s.choices()))
        .collect();
    format!(
        "Set one or more AC controls in a single command, e.g. \
         {{\"turbo\": true, \"airflow\": \"circulate\", \"horizontal\": \"left-center\", \"low_watt\": \"3\"}}. \
         Airflow, a vertical direction and gentle exclude each other; the AC keeps the last one set. \
         Writable controls:\n{}",
        guide.join("\n")
    )
}

#[derive(Clone)]
pub struct AirconServer {
    tool_router: ToolRouter<Self>,
    // One AC request at a time, so a write and its read-back never interleave
    // with another call's.
    lock: Arc<Mutex<()>>,
}

impl AirconServer {
    fn run(
        &self,
        action: impl FnOnce(&mut AirConditioner) -> anyhow::Result<Map<String, Value>>,
    ) -> Result<String, String> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = AirConditioner::open().and_then(|mut ac| action(&mut ac));
        match result {
            Ok(values) => Ok(serde_json::to_string_pretty(&values).unwrap_or_default()),
            Err(error) => Err(format!("{error:#}")),
        }
    }

    fn set(&self, name: &str, value: Value) -> Result<String, String> {
        self.run(|ac| ac.set(&[(name.to_string(), value)]))
    }
}

#[tool_router]
impl AirconServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Read the AC's full state: power, mode, target and room temperature, fan speed,
    /// AI energy saving, turbo, low watt, gentle, airflow, louvres, display, outdoor
    /// temperature, energy use (kWh) and fault code.
    #[tool]
    fn get_status(&self) -> Result<String, String> {
        self.run(|ac| ac.status(&[]))
    }

    /// Turn the AC on or off.
    #[tool]
    fn set_power(
        &self,
        Parameters(PowerArgs { on }): Parameters<PowerArgs>,
    ) -> Result<String, String> {
        self.set("power", Value::Bool(on))
    }

    /// Set the target temperature, 16-31 °C in 0.5 steps.
    #[tool]
    fn set_temperature(
        &self,
        Parameters(TemperatureArgs { celsius }): Parameters<TemperatureArgs>,
    ) -> Result<String, String> {
        self.set("target_temperature", Value::from(celsius))
    }

    /// Set the operating mode. This unit cools only; there is no heat or auto mode.
    #[tool]
    fn set_mode(
        &self,
        Parameters(ModeArgs { mode }): Parameters<ModeArgs>,
    ) -> Result<String, String> {
        let mode = match mode {
            Mode::Cool => "cool",
            Mode::Dry => "dry",
            Mode::Fan => "fan",
        };
        self.set("mode", Value::from(mode))
    }

    /// Set the fan speed: auto, or 1 (quietest) to 7 (strongest). Turns turbo off.
    #[tool]
    fn set_fan_speed(
        &self,
        Parameters(FanArgs { speed }): Parameters<FanArgs>,
    ) -> Result<String, String> {
        self.set("fan_speed", Value::from(speed))
    }

    #[tool(description = controls_description())]
    fn set_controls(
        &self,
        Parameters(ControlsArgs { values }): Parameters<ControlsArgs>,
    ) -> Result<String, String> {
        let values: Vec<(String, Value)> = values.into_iter().collect();
        self.run(|ac| ac.set(&values))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AirconServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Controls the Mijia air conditioner on the local network.")
    }
}

pub fn serve() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let service = AirconServer::new().serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}
