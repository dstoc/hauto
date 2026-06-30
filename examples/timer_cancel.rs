//! Demonstrate delayed action cancellation/replacement with motion lighting.
//!
//! Motion turns the light on and cancels any pending off timer. Motion becoming
//! off schedules a 30 second delayed turn-off. If motion returns before the
//! delay expires, the pending timer is cancelled and replaced by the next
//! motion-off event.
//!
//! Required environment variables:
//!
//! - `HOME_ASSISTANT_URL`: Home Assistant base URL, for example `http://homeassistant.local:8123`
//! - `HOME_ASSISTANT_TOKEN`: long-lived access token
//! - `HAUTO_MOTION_SENSOR`: binary sensor entity id, for example `binary_sensor.hall_motion`
//! - `HAUTO_LIGHT`: light entity id, for example `light.hall`
//!
//! Run with:
//!
//! ```sh
//! cargo run --example timer_cancel
//! ```

use std::{env, error::Error, time::Duration};

use hauto::{
    App, BinarySensor, Error as HautoError, Light, LightTurnOff, LightTurnOn, TimerHandle,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;
    let motion = BinarySensor::new(required_env("HAUTO_MOTION_SENSOR")?)?;
    let light = Light::new(required_env("HAUTO_LIGHT")?)?;

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("motion light with cancellable off timer", move |ctx| {
            let motion = motion.clone();
            let light = light.clone();

            async move {
                let mut changes = ctx.binary_sensor_changes(&motion);
                let mut pending_off: Option<TimerHandle<()>> = None;

                while let Some(event) = changes.next().await {
                    let event = event.map_err(HautoError::EventStream)?;
                    let Some(new_state) = event.new_state else {
                        continue;
                    };

                    match new_state.state.as_str() {
                        "on" => {
                            if let Some(mut timer) = pending_off.take() {
                                timer.cancel().await?;
                            }
                            light.turn_on(&ctx, LightTurnOn::default()).await?;
                        }
                        "off" => {
                            if let Some(mut timer) = pending_off.take() {
                                timer.cancel().await?;
                            }

                            let timer_ctx = ctx.clone();
                            let timer_light = light.clone();
                            pending_off =
                                Some(ctx.run_after(Duration::from_secs(30), async move {
                                    timer_light
                                        .turn_off(&timer_ctx, LightTurnOff::default())
                                        .await?;
                                    Ok(())
                                }));
                        }
                        _ => {}
                    }
                }

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
