//! Turn a light on while a binary occupancy sensor is occupied, then turn it
//! off after the area has been clear for 30 seconds.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_OCCUPANCY_SENSOR`: binary sensor entity id, for example `binary_sensor.office_occupancy`
//! - `HAUTO_LIGHT`: light entity id, for example `light.office`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example occupancy_light
//! ```

use std::{env, error::Error, time::Duration};

use hauto::{App, BinarySensor, HoldResult, Light, LightTurnOff, LightTurnOn};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let occupancy = BinarySensor::new(required_env("HAUTO_OCCUPANCY_SENSOR")?)?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("occupancy-controlled light", move |ctx| {
            let occupancy = occupancy.clone();
            let light = light.clone();

            async move {
                let mut require_transition = false;

                loop {
                    if require_transition {
                        occupancy.wait_until_on(&ctx).require_transition().await?;
                    } else {
                        occupancy.wait_until_on(&ctx).await?;
                    }
                    require_transition = true;

                    light.turn_on(&ctx, LightTurnOn::default()).await?;
                    occupancy.wait_until_off(&ctx).await?;

                    match occupancy
                        .expect_off(&ctx)
                        .for_at_least(Duration::from_secs(30))
                        .await?
                    {
                        HoldResult::Held => {
                            light.turn_off(&ctx, LightTurnOff::default()).await?;
                        }
                        HoldResult::NotSatisfied { .. } | HoldResult::Interrupted { .. } => {
                            require_transition = false;
                        }
                    }
                }
            }
        })
        .run()
        .await?;

    Ok(())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("missing required environment variable `{name}`").into())
}
