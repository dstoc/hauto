//! Sketch conversion of the AppDaemon `AppliancePowerStatus` automation.
//!
//! This file is the reusable automation logic. Copy it into another hauto
//! project and construct [`AppliancePowerStatusConfig`] values from that
//! project's own config/bootstrap code.

use std::time::Duration;

use hauto::{
    Context, EntityId, EntityState, Error as HautoError, Sensor, StateChangeStream, StateWrite,
};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct AppliancePowerStatusConfig {
    pub power_entity: Sensor<f64>,
    pub status_entity: EntityId,
    pub friendly_name: String,
    pub off_below: f64,
    pub idle_below: f64,
    pub off_delay: Duration,
    pub idle_delay: Duration,
    pub icons: StatusIcons,
}

#[derive(Clone, Debug)]
pub struct StatusIcons {
    pub off: String,
    pub idle: String,
    pub running: String,
    pub unknown: String,
}

impl StatusIcons {
    fn icon_for(&self, status: ApplianceStatus) -> &str {
        match status {
            ApplianceStatus::Off => &self.off,
            ApplianceStatus::Idle => &self.idle,
            ApplianceStatus::Running => &self.running,
            ApplianceStatus::Unknown => &self.unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplianceStatus {
    Off,
    Idle,
    Running,
    Unknown,
}

impl ApplianceStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Unknown => "unknown",
        }
    }
}

pub struct AppliancePowerStatus {
    config: AppliancePowerStatusConfig,
}

impl AppliancePowerStatus {
    pub fn new(config: AppliancePowerStatusConfig) -> Self {
        Self { config }
    }

    pub async fn run(self, ctx: Context) -> hauto::Result<()> {
        let mut changes = ctx.state_changes(self.config.power_entity.entity_id());

        loop {
            let Some(power) = read_power(&ctx, &self.config.power_entity).await? else {
                set_status(&ctx, &self.config, ApplianceStatus::Unknown).await?;
                next_power_change(&ctx, &mut changes).await?;
                continue;
            };

            if power >= self.config.idle_below {
                set_status(&ctx, &self.config, ApplianceStatus::Running).await?;
                next_power_change(&ctx, &mut changes).await?;
                continue;
            }

            if power >= self.config.off_below {
                if hold_power_below(
                    &ctx,
                    &mut changes,
                    self.config.idle_below,
                    self.config.idle_delay,
                )
                .await?
                {
                    set_status(&ctx, &self.config, ApplianceStatus::Idle).await?;
                    next_power_change(&ctx, &mut changes).await?;
                }
                continue;
            }

            if hold_power_below(
                &ctx,
                &mut changes,
                self.config.off_below,
                self.config.off_delay,
            )
            .await?
            {
                set_status(&ctx, &self.config, ApplianceStatus::Off).await?;
                next_power_change(&ctx, &mut changes).await?;
            }
        }
    }
}

async fn read_power(ctx: &Context, power_entity: &Sensor<f64>) -> hauto::Result<Option<f64>> {
    match power_entity.state(ctx).await {
        Ok(state) => Ok(to_float(&state)),
        Err(HautoError::EntityNotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn set_status(
    ctx: &Context,
    config: &AppliancePowerStatusConfig,
    status: ApplianceStatus,
) -> hauto::Result<()> {
    let state = status.as_str();
    ctx.home_assistant()
        .set_state_raw(
            &config.status_entity,
            StateWrite::new(
                state,
                json!({
                    "friendly_name": &config.friendly_name,
                    "icon": config.icons.icon_for(status),
                }),
            )?,
        )
        .await?;
    Ok(())
}

async fn hold_power_below(
    ctx: &Context,
    changes: &mut StateChangeStream,
    threshold: f64,
    duration: Duration,
) -> hauto::Result<bool> {
    if duration.is_zero() {
        return Ok(true);
    }

    let deadline = tokio::time::sleep(duration);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            () = &mut deadline => return Ok(true),
            power = next_power_change(ctx, changes) => {
                let Some(power) = power? else {
                    return Ok(false);
                };
                if power >= threshold {
                    return Ok(false);
                }
            }
        }
    }
}

async fn next_power_change(
    ctx: &Context,
    changes: &mut StateChangeStream,
) -> hauto::Result<Option<f64>> {
    tokio::select! {
        event = changes.next() => {
            let event = event
                .ok_or_else(|| HautoError::Connection("power state change stream closed".to_string()))?
                .map_err(HautoError::EventStream)?;
            Ok(event.new_state.as_ref().and_then(to_float))
        }
        () = ctx.cancelled() => Err(HautoError::Cancelled),
    }
}

fn to_float(state: &EntityState) -> Option<f64> {
    to_float_str(&state.state)
}

fn to_float_str(value: &str) -> Option<f64> {
    match value {
        "" | "unknown" | "unavailable" => None,
        value => value.parse::<f64>().ok(),
    }
}
