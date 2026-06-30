//! Call a Home Assistant service through the raw service escape hatch.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_SERVICE_DOMAIN`: service domain, for example `notify`
//! - `HAUTO_SERVICE_NAME`: service name, for example `persistent_notification` or a mobile app notify service name
//! - `HAUTO_SERVICE_DATA`: JSON object string, for example `{"message":"hello from hauto"}`
//!
//! Run with:
//!
//! ```sh
//! HAUTO_SERVICE_DATA='{"message":"hello from hauto"}' cargo run --example raw_service
//! ```

use std::{env, error::Error};

use hauto::App;
use serde_json::Value;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let domain = required_env("HAUTO_SERVICE_DOMAIN")?;
    let service = required_env("HAUTO_SERVICE_NAME")?;
    let data = parse_service_data(&required_env("HAUTO_SERVICE_DATA")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("call raw service once", move |ctx| {
            let domain = domain.clone();
            let service = service.clone();
            let data = data.clone();

            async move {
                let response = ctx
                    .home_assistant()
                    .call_service_raw(&domain, &service, data)
                    .await?;

                println!("{response}");
                Ok(())
            }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}

fn parse_service_data(data: &str) -> Result<Value, Box<dyn Error>> {
    let value: Value = serde_json::from_str(data)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err("`HAUTO_SERVICE_DATA` must be a JSON object".into())
    }
}
