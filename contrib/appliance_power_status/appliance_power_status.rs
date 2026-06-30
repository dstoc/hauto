//! Sketch conversion of the AppDaemon `AppliancePowerStatus` automation.
//!
//! This file is the reusable automation logic. Copy it into another hauto
//! project and construct [`AppliancePowerStatusConfig`] values from that
//! project's own config/bootstrap code.

use std::time::Duration;

use hauto::{Context, EntityId, Error as HautoError, HoldResult, Sensor, SensorValue, StateWrite};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct AppliancePowerStatusConfig {
    pub power_entity: Sensor<SensorValue<f64>>,
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
        loop {
            let Some(power) = read_power(&ctx, &self.config.power_entity).await? else {
                set_status(&ctx, &self.config, ApplianceStatus::Unknown).await?;
                next_power_change(&ctx, &self.config.power_entity).await?;
                continue;
            };

            if power >= self.config.idle_below {
                set_status(&ctx, &self.config, ApplianceStatus::Running).await?;
                next_power_change(&ctx, &self.config.power_entity).await?;
                continue;
            }

            if power >= self.config.off_below {
                if power_held_below(
                    &ctx,
                    &self.config.power_entity,
                    self.config.idle_below,
                    self.config.idle_delay,
                )
                .await?
                {
                    set_status(&ctx, &self.config, ApplianceStatus::Idle).await?;
                    next_power_change(&ctx, &self.config.power_entity).await?;
                }
                continue;
            }

            if power_held_below(
                &ctx,
                &self.config.power_entity,
                self.config.off_below,
                self.config.off_delay,
            )
            .await?
            {
                set_status(&ctx, &self.config, ApplianceStatus::Off).await?;
                next_power_change(&ctx, &self.config.power_entity).await?;
            }
        }
    }
}

async fn read_power(
    ctx: &Context,
    power_entity: &Sensor<SensorValue<f64>>,
) -> hauto::Result<Option<f64>> {
    match power_entity.get(ctx).await {
        Ok(value) => Ok(value.into_value()),
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

async fn power_held_below(
    ctx: &Context,
    power_entity: &Sensor<SensorValue<f64>>,
    threshold: f64,
    duration: Duration,
) -> hauto::Result<bool> {
    Ok(matches!(
        power_entity
            .expect_matching(ctx, move |value| {
                value.as_value().is_some_and(|power| *power < threshold)
            })
            .for_at_least(duration)
            .await?,
        HoldResult::Held
    ))
}

async fn next_power_change(
    ctx: &Context,
    power_entity: &Sensor<SensorValue<f64>>,
) -> hauto::Result<()> {
    let mut changes = ctx.state_changes(power_entity.entity_id());

    tokio::select! {
        event = changes.next() => {
            event
                .ok_or_else(|| HautoError::Connection("power state change stream closed".to_string()))?
                .map_err(HautoError::EventStream)?;
            Ok(())
        }
        () = ctx.cancelled() => Err(HautoError::Cancelled),
    }
}
