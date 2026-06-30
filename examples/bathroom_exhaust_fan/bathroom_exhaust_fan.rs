//! Bathroom exhaust fan control for two bathrooms sharing one fan.
//!
//! The automation treats humidity as the safety trigger and occupancy as a
//! comfort trigger outside quiet hours.

use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hauto::{
    BinarySensor, BinaryState, Context, EntityId, Error as HautoError, Sensor, SensorValue,
};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct BathroomExhaustFanConfig {
    pub fan_entity: EntityId,
    pub ambient: AmbientSensors,
    pub bathrooms: [BathroomConfig; 2],
    pub settings: Settings,
}

#[derive(Clone, Debug)]
pub struct AmbientSensors {
    pub temperature: Sensor<SensorValue<f64>>,
    pub humidity: Sensor<SensorValue<f64>>,
}

#[derive(Clone, Debug)]
pub struct BathroomConfig {
    pub name: String,
    pub temperature: Sensor<SensorValue<f64>>,
    pub humidity: Sensor<SensorValue<f64>>,
    pub occupancy: BinarySensor,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub quiet_hours: QuietHours,
    pub occupancy_post_run: Duration,
    pub humidity_clear_hold: Duration,
    pub humidity_minimum_run: Duration,
    pub humidity_maximum_run: Duration,
    pub minimum_on_time: Duration,
    pub minimum_off_time: Duration,
    pub poll_interval: Duration,
    pub absolute_humidity_start_excess: f64,
    pub absolute_humidity_clear_excess: f64,
    pub relative_humidity_start_excess: f64,
    pub relative_humidity_clear_excess: f64,
    pub relative_humidity_start: f64,
    pub relative_humidity_clear: f64,
    pub relative_humidity_extreme: f64,
    pub absolute_humidity_rise_window: Duration,
    pub absolute_humidity_rise_start: f64,
    pub relative_humidity_rise_start: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quiet_hours: QuietHours {
                start_minute: 0,
                end_minute: 8 * 60,
                utc_offset_minutes: 0,
            },
            occupancy_post_run: Duration::from_secs(7 * 60),
            humidity_clear_hold: Duration::from_secs(5 * 60),
            humidity_minimum_run: Duration::from_secs(10 * 60),
            humidity_maximum_run: Duration::from_secs(90 * 60),
            minimum_on_time: Duration::from_secs(2 * 60),
            minimum_off_time: Duration::from_secs(60),
            poll_interval: Duration::from_secs(30),
            absolute_humidity_start_excess: 2.0,
            absolute_humidity_clear_excess: 0.8,
            relative_humidity_start_excess: 12.0,
            relative_humidity_clear_excess: 6.0,
            relative_humidity_start: 70.0,
            relative_humidity_clear: 65.0,
            relative_humidity_extreme: 80.0,
            absolute_humidity_rise_window: Duration::from_secs(5 * 60),
            absolute_humidity_rise_start: 0.6,
            relative_humidity_rise_start: 6.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QuietHours {
    /// Minute of local day when quiet hours start, e.g. midnight is `0`.
    pub start_minute: u32,
    /// Minute of local day when quiet hours end, e.g. 08:00 is `480`.
    pub end_minute: u32,
    /// Fixed local offset from UTC in minutes.
    pub utc_offset_minutes: i32,
}

impl QuietHours {
    fn is_quiet_now(self) -> bool {
        let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return false;
        };
        let utc_seconds = elapsed.as_secs() as i128;
        let local_seconds = utc_seconds + i128::from(self.utc_offset_minutes) * 60;
        let day_seconds = local_seconds.rem_euclid(24 * 60 * 60);
        let minute = u32::try_from(day_seconds / 60).expect("minute of day fits in u32");

        if self.start_minute <= self.end_minute {
            self.start_minute <= minute && minute < self.end_minute
        } else {
            self.start_minute <= minute || minute < self.end_minute
        }
    }
}

pub struct BathroomExhaustFan {
    config: BathroomExhaustFanConfig,
    bathrooms: [BathroomRuntime; 2],
    fan_on: bool,
    last_on: Option<Instant>,
    last_off: Option<Instant>,
}

impl BathroomExhaustFan {
    pub fn new(config: BathroomExhaustFanConfig) -> Self {
        Self {
            config,
            bathrooms: [BathroomRuntime::default(), BathroomRuntime::default()],
            fan_on: false,
            last_on: None,
            last_off: None,
        }
    }

    pub async fn run(mut self, ctx: Context) -> hauto::Result<()> {
        loop {
            let now = Instant::now();
            let quiet = self.config.settings.quiet_hours.is_quiet_now();

            for index in 0..self.bathrooms.len() {
                let inputs =
                    read_bathroom_inputs(&ctx, &self.config.ambient, &self.config.bathrooms[index])
                        .await?;
                self.bathrooms[index].record_sample(now, &inputs, &self.config.settings);
                let previous_state = self.bathrooms[index].state;
                self.bathrooms[index].advance(now, quiet, &inputs, &self.config.settings);
                if self.bathrooms[index].state != previous_state {
                    println!(
                        "{}: {:?} -> {:?}",
                        self.config.bathrooms[index].name,
                        previous_state,
                        self.bathrooms[index].state
                    );
                }
            }

            self.apply_fan(&ctx, now, quiet).await?;
            self.wait_for_relevant_change_or_tick(&ctx).await?;
        }
    }

    async fn apply_fan(&mut self, ctx: &Context, now: Instant, quiet: bool) -> hauto::Result<()> {
        let humidity_demand = self
            .bathrooms
            .iter()
            .any(|bathroom| bathroom.state.is_humid());
        let desired_on = if quiet {
            humidity_demand
        } else {
            self.bathrooms
                .iter()
                .any(|bathroom| !matches!(bathroom.state, BathroomState::Idle))
        };
        let force_quiet_occupancy_off = quiet && !humidity_demand;

        if desired_on && !self.fan_on {
            if self.last_off.is_some_and(|last| {
                now.duration_since(last) < self.config.settings.minimum_off_time
            }) {
                return Ok(());
            }

            turn_entity(ctx, &self.config.fan_entity, true).await?;
            self.fan_on = true;
            self.last_on = Some(now);
        } else if !desired_on && self.fan_on {
            if !force_quiet_occupancy_off
                && self.last_on.is_some_and(|last| {
                    now.duration_since(last) < self.config.settings.minimum_on_time
                })
            {
                return Ok(());
            }

            turn_entity(ctx, &self.config.fan_entity, false).await?;
            self.fan_on = false;
            self.last_off = Some(now);
        }

        Ok(())
    }

    async fn wait_for_relevant_change_or_tick(&self, ctx: &Context) -> hauto::Result<()> {
        let bathroom_1 = &self.config.bathrooms[0];
        let bathroom_2 = &self.config.bathrooms[1];
        let ambient = &self.config.ambient;

        tokio::select! {
            result = bathroom_1.temperature.next_change(ctx) => ignore_sensor_change(result),
            result = bathroom_1.humidity.next_change(ctx) => ignore_sensor_change(result),
            result = bathroom_1.occupancy.next_change(ctx) => ignore_binary_change(result),
            result = bathroom_2.temperature.next_change(ctx) => ignore_sensor_change(result),
            result = bathroom_2.humidity.next_change(ctx) => ignore_sensor_change(result),
            result = bathroom_2.occupancy.next_change(ctx) => ignore_binary_change(result),
            result = ambient.temperature.next_change(ctx) => ignore_sensor_change(result),
            result = ambient.humidity.next_change(ctx) => ignore_sensor_change(result),
            result = ctx.sleep(self.config.settings.poll_interval) => result,
        }
    }
}

#[derive(Default)]
struct BathroomRuntime {
    state: BathroomState,
    samples: VecDeque<HumiditySample>,
}

impl BathroomRuntime {
    fn record_sample(&mut self, now: Instant, inputs: &BathroomInputs, settings: &Settings) {
        self.samples.push_back(HumiditySample {
            at: now,
            absolute_humidity: inputs.bathroom_absolute_humidity,
            relative_humidity: inputs.bathroom_relative_humidity,
        });

        while self.samples.front().is_some_and(|sample| {
            now.duration_since(sample.at) > settings.absolute_humidity_rise_window * 2
        }) {
            self.samples.pop_front();
        }
    }

    fn advance(&mut self, now: Instant, quiet: bool, inputs: &BathroomInputs, settings: &Settings) {
        let humidity_start =
            inputs.humidity_start(settings) || humidity_rise_started(&self.samples, now, settings);

        self.state = match self.state {
            BathroomState::Idle => {
                if humidity_start {
                    BathroomState::Humid {
                        started_at: now,
                        clear_since: None,
                    }
                } else if inputs.occupied && !quiet {
                    BathroomState::Occupied
                } else {
                    BathroomState::Idle
                }
            }
            BathroomState::Occupied => {
                if humidity_start {
                    BathroomState::Humid {
                        started_at: now,
                        clear_since: None,
                    }
                } else if quiet {
                    BathroomState::Idle
                } else if !inputs.occupied {
                    BathroomState::Drying {
                        until: now + settings.occupancy_post_run,
                    }
                } else {
                    BathroomState::Occupied
                }
            }
            BathroomState::Drying { until } => {
                if humidity_start {
                    BathroomState::Humid {
                        started_at: now,
                        clear_since: None,
                    }
                } else if quiet {
                    BathroomState::Idle
                } else if inputs.occupied {
                    BathroomState::Occupied
                } else if now >= until {
                    BathroomState::Idle
                } else {
                    BathroomState::Drying { until }
                }
            }
            BathroomState::Humid {
                started_at,
                clear_since,
            } => {
                let clear_since = if inputs.humidity_clear(settings) {
                    Some(clear_since.unwrap_or(now))
                } else {
                    None
                };

                let minimum_elapsed =
                    now.duration_since(started_at) >= settings.humidity_minimum_run;
                let clear_held = clear_since.is_some_and(|clear_since| {
                    now.duration_since(clear_since) >= settings.humidity_clear_hold
                });
                let maximum_elapsed =
                    now.duration_since(started_at) >= settings.humidity_maximum_run;

                let should_stop_for_clear = minimum_elapsed && clear_held;
                let should_stop_for_maximum = maximum_elapsed && !inputs.extremely_humid(settings);

                if should_stop_for_clear || should_stop_for_maximum {
                    BathroomState::Idle
                } else {
                    BathroomState::Humid {
                        started_at,
                        clear_since,
                    }
                }
            }
        };
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BathroomState {
    #[default]
    Idle,
    Occupied,
    Drying {
        until: Instant,
    },
    Humid {
        started_at: Instant,
        clear_since: Option<Instant>,
    },
}

impl BathroomState {
    fn is_humid(self) -> bool {
        matches!(self, Self::Humid { .. })
    }
}

#[derive(Clone, Copy)]
struct HumiditySample {
    at: Instant,
    absolute_humidity: Option<f64>,
    relative_humidity: Option<f64>,
}

struct BathroomInputs {
    occupied: bool,
    bathroom_relative_humidity: Option<f64>,
    ambient_relative_humidity: Option<f64>,
    bathroom_absolute_humidity: Option<f64>,
    ambient_absolute_humidity: Option<f64>,
}

impl BathroomInputs {
    fn humidity_start(&self, settings: &Settings) -> bool {
        self.absolute_humidity_excess()
            .is_some_and(|excess| excess > settings.absolute_humidity_start_excess)
            || self
                .relative_humidity_excess()
                .is_some_and(|excess| excess > settings.relative_humidity_start_excess)
            || self
                .bathroom_relative_humidity
                .is_some_and(|humidity| humidity > settings.relative_humidity_start)
    }

    fn humidity_clear(&self, settings: &Settings) -> bool {
        self.absolute_humidity_excess()
            .is_some_and(|excess| excess < settings.absolute_humidity_clear_excess)
            && self
                .relative_humidity_excess()
                .is_some_and(|excess| excess < settings.relative_humidity_clear_excess)
            && self
                .bathroom_relative_humidity
                .is_some_and(|humidity| humidity < settings.relative_humidity_clear)
    }

    fn extremely_humid(&self, settings: &Settings) -> bool {
        self.bathroom_relative_humidity
            .is_some_and(|humidity| humidity > settings.relative_humidity_extreme)
    }

    fn absolute_humidity_excess(&self) -> Option<f64> {
        Some(self.bathroom_absolute_humidity? - self.ambient_absolute_humidity?)
    }

    fn relative_humidity_excess(&self) -> Option<f64> {
        Some(self.bathroom_relative_humidity? - self.ambient_relative_humidity?)
    }
}

async fn read_bathroom_inputs(
    ctx: &Context,
    ambient: &AmbientSensors,
    bathroom: &BathroomConfig,
) -> hauto::Result<BathroomInputs> {
    let bathroom_temperature = bathroom.temperature.get(ctx).await?.into_value();
    let bathroom_relative_humidity = bathroom.humidity.get(ctx).await?.into_value();
    let ambient_temperature = ambient.temperature.get(ctx).await?.into_value();
    let ambient_relative_humidity = ambient.humidity.get(ctx).await?.into_value();
    let occupied = match bathroom.occupancy.get(ctx).await {
        Ok(BinaryState::On) => true,
        Ok(BinaryState::Off | BinaryState::Unknown | BinaryState::Unavailable) => false,
        Err(HautoError::EntityNotFound(_)) => false,
        Err(error) => return Err(error),
    };

    Ok(BathroomInputs {
        occupied,
        bathroom_relative_humidity,
        ambient_relative_humidity,
        bathroom_absolute_humidity: absolute_humidity(
            bathroom_temperature,
            bathroom_relative_humidity,
        ),
        ambient_absolute_humidity: absolute_humidity(
            ambient_temperature,
            ambient_relative_humidity,
        ),
    })
}

fn absolute_humidity(temperature_c: Option<f64>, relative_humidity: Option<f64>) -> Option<f64> {
    let temperature_c = temperature_c?;
    let relative_humidity = relative_humidity?;
    let saturation_vapor_pressure_hpa =
        6.112 * ((17.67 * temperature_c) / (temperature_c + 243.5)).exp();
    Some(
        216.7 * (relative_humidity / 100.0 * saturation_vapor_pressure_hpa)
            / (temperature_c + 273.15),
    )
}

fn humidity_rise_started(
    samples: &VecDeque<HumiditySample>,
    now: Instant,
    settings: &Settings,
) -> bool {
    let Some(current) = samples.back() else {
        return false;
    };

    samples
        .iter()
        .filter(|sample| now.duration_since(sample.at) >= settings.absolute_humidity_rise_window)
        .any(|sample| {
            let absolute_rise = current
                .absolute_humidity
                .zip(sample.absolute_humidity)
                .is_some_and(|(current, old)| {
                    current - old > settings.absolute_humidity_rise_start
                });
            let relative_rise = current
                .relative_humidity
                .zip(sample.relative_humidity)
                .is_some_and(|(current, old)| {
                    current - old > settings.relative_humidity_rise_start
                });
            absolute_rise || relative_rise
        })
}

async fn turn_entity(ctx: &Context, entity_id: &EntityId, on: bool) -> hauto::Result<()> {
    let service = if on { "turn_on" } else { "turn_off" };
    ctx.home_assistant()
        .call_service_raw(
            entity_id.domain(),
            service,
            json!({
                "entity_id": entity_id.as_str(),
            }),
        )
        .await?;
    Ok(())
}

fn ignore_sensor_change(result: hauto::Result<SensorValue<f64>>) -> hauto::Result<()> {
    match result {
        Ok(_) | Err(HautoError::EntityNotFound(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

fn ignore_binary_change(result: hauto::Result<BinaryState>) -> hauto::Result<()> {
    match result {
        Ok(_) | Err(HautoError::EntityNotFound(_)) => Ok(()),
        Err(error) => Err(error),
    }
}
