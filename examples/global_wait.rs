//! Turn a light on after temperature and humidity thresholds are satisfied
//! at the same time.
//!
//! This demonstrates global state predicate waits over multiple entities with
//! `Sensor::<f64>::read`.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_TEMPERATURE_SENSOR`: sensor entity id, for example `sensor.office_temperature`
//! - `HAUTO_HUMIDITY_SENSOR`: sensor entity id, for example `sensor.office_humidity`
//! - `HAUTO_TEMPERATURE_THRESHOLD`: numeric threshold, for example `24.5`
//! - `HAUTO_HUMIDITY_THRESHOLD`: numeric threshold, for example `55.0`
//! - `HAUTO_LIGHT`: light entity id, for example `light.office`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example global_wait
//! ```

use std::{env, error::Error, time::Duration};

use hauto::{App, Light, LightTurnOn, Sensor};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let temperature = Sensor::<f64>::new(required_env("HAUTO_TEMPERATURE_SENSOR")?)?;
    let humidity = Sensor::<f64>::new(required_env("HAUTO_HUMIDITY_SENSOR")?)?;
    let temperature_threshold = required_env("HAUTO_TEMPERATURE_THRESHOLD")?.parse::<f64>()?;
    let humidity_threshold = required_env("HAUTO_HUMIDITY_THRESHOLD")?.parse::<f64>()?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn(
            "global temperature and humidity threshold light",
            move |ctx| {
                let temperature = temperature.clone();
                let humidity = humidity.clone();
                let light = light.clone();

                async move {
                    println!(
                        "Waiting for {} >= {temperature_threshold} and {} <= {humidity_threshold}",
                        temperature.entity_id(),
                        humidity.entity_id()
                    );

                    ctx.wait_until_state(move |state| {
                        let Some(temperature) = temperature.read(state)? else {
                            return Ok(false);
                        };
                        let Some(humidity) = humidity.read(state)? else {
                            return Ok(false);
                        };

                        Ok(temperature >= temperature_threshold && humidity <= humidity_threshold)
                    })
                    .for_at_least(Duration::from_secs(30))
                    .await?;

                    println!(
                        "Thresholds held for 30 seconds; turning {} on",
                        light.entity_id()
                    );
                    light.turn_on(&ctx, LightTurnOn::default()).await?;

                    Ok(())
                }
            },
        )
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
