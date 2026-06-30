//! Runnable wrapper for the reusable appliance power status automation.
//!
//! The automation logic lives in `appliance_power_status.rs` so it can be copied
//! into another hauto project without also copying this example bootstrap.

use std::time::Duration;

use appliance_power_status::{AppliancePowerStatus, AppliancePowerStatusConfig, StatusIcons};
use hauto::{App, EntityId, Error as HautoError, Sensor, SensorValue};

mod appliance_power_status;

#[tokio::main(flavor = "current_thread")]
async fn main() -> hauto::Result<()> {
    let home_assistant_url = required_env("HOME_ASSISTANT_URL")?;
    let home_assistant_token = required_env("HOME_ASSISTANT_TOKEN")?;

    let washing_machine = AppliancePowerStatusConfig {
        power_entity: Sensor::<SensorValue<f64>>::new("sensor.laundry_washing_machine_power")?,
        status_entity: EntityId::new("sensor.washing_machine_status")?,
        friendly_name: "Washing Machine Status".to_string(),
        off_below: 3.0,
        idle_below: 10.0,
        off_delay: Duration::from_secs(300),
        idle_delay: Duration::from_secs(30),
        icons: StatusIcons {
            off: "mdi:washing-machine-off".to_string(),
            idle: "mdi:pause-circle".to_string(),
            running: "mdi:washing-machine".to_string(),
            unknown: "mdi:help-circle".to_string(),
        },
    };

    let dryer = AppliancePowerStatusConfig {
        power_entity: Sensor::<SensorValue<f64>>::new("sensor.laundry_dryer_power")?,
        status_entity: EntityId::new("sensor.dryer_status")?,
        friendly_name: "Dryer Status".to_string(),
        off_below: 1.0,
        idle_below: 10.0,
        off_delay: Duration::from_secs(300),
        idle_delay: Duration::from_secs(30),
        icons: StatusIcons {
            off: "mdi:tumble-dryer-off".to_string(),
            idle: "mdi:pause-circle".to_string(),
            running: "mdi:tumble-dryer".to_string(),
            unknown: "mdi:help-circle".to_string(),
        },
    };

    App::new(home_assistant_url, home_assistant_token)
        .automation_fn("washing machine status", move |ctx| {
            let automation = AppliancePowerStatus::new(washing_machine.clone());
            async move { automation.run(ctx).await }
        })
        .automation_fn("dryer status", move |ctx| {
            let automation = AppliancePowerStatus::new(dryer.clone());
            async move { automation.run(ctx).await }
        })
        .run()
        .await
}

fn required_env(name: &'static str) -> hauto::Result<String> {
    std::env::var(name).map_err(|_| {
        HautoError::InvalidServiceOptions(format!("missing required environment variable `{name}`"))
    })
}
