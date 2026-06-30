//! Turn a light on or off from a numeric temperature sensor threshold.
//!
//! This demonstrates typed predicate waits with `Sensor::<f64>`.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_TEMPERATURE_SENSOR`: sensor entity id, for example `sensor.office_temperature`
//! - `HAUTO_THRESHOLD`: numeric threshold, for example `24.5`
//! - `HAUTO_LIGHT`: light entity id, for example `light.office`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example temperature_threshold
//! ```

use std::{env, error::Error};

use hauto::{App, Context, Light, LightTurnOff, LightTurnOn, Sensor};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let sensor = Sensor::<f64>::new(required_env("HAUTO_TEMPERATURE_SENSOR")?)?;
    let threshold = required_env("HAUTO_THRESHOLD")?.parse::<f64>()?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("temperature threshold light control", move |ctx| {
            let sensor = sensor.clone();
            let light = light.clone();

            async move {
                loop {
                    sensor
                        .wait_until_matching(&ctx, move |temperature| *temperature >= threshold)
                        .await?;
                    set_light_for_threshold(&ctx, &sensor, &light, threshold, true).await?;

                    sensor
                        .wait_until_matching(&ctx, move |temperature| *temperature < threshold)
                        .await?;
                    set_light_for_threshold(&ctx, &sensor, &light, threshold, false).await?;
                }
            }
        })
        .run()
        .await?;

    Ok(())
}

async fn set_light_for_threshold(
    ctx: &Context,
    sensor: &Sensor<f64>,
    light: &Light,
    threshold: f64,
    above_or_equal: bool,
) -> hauto::Result<()> {
    if above_or_equal {
        println!(
            "{} reached >= {threshold}; turning {} on",
            sensor.entity_id(),
            light.entity_id()
        );
        light.turn_on(ctx, LightTurnOn::default()).await?;
    } else {
        println!(
            "{} dropped below {threshold}; turning {} off",
            sensor.entity_id(),
            light.entity_id()
        );
        light.turn_off(ctx, LightTurnOff::default()).await?;
    }

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
