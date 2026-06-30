//! Send one raw Home Assistant WebSocket command.
//!
//! This is an advanced escape-hatch example. The command is supplied as a JSON
//! object and passed through to Home Assistant without a caller-supplied `id`;
//! the hauto WebSocket client assigns the request id internally.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_COMMAND`: JSON object command, for example `{"type":"get_config"}`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example raw_command
//! ```

use std::{env, error::Error};

use hauto::App;
use serde_json::Value;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let command = parse_command(&required_env("HAUTO_COMMAND")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("send one raw command", move |ctx| {
            let command = command.clone();

            async move {
                let result = ctx.home_assistant().command_raw(command).await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .expect("serializing a serde_json::Value should not fail")
                );
                Ok(())
            }
        })
        .run()
        .await?;

    Ok(())
}

fn parse_command(command: &str) -> Result<Value, Box<dyn Error>> {
    let command: Value = serde_json::from_str(command)?;
    if !command.is_object() {
        return Err("`HAUTO_COMMAND` must be a JSON object".into());
    }
    Ok(command)
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
