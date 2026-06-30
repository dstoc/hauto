//! Per-bathroom humidity status publisher.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use hauto::{Context, EntityId, Error as HautoError, Sensor, SensorValue, StateWrite};
use serde_json::json;

#[derive(Clone, Debug)]
pub struct HumidityStatusConfig {
    pub name: String,
    pub status_entity: EntityId,
    pub bathroom_temperature: Sensor<SensorValue<f64>>,
    pub bathroom_humidity: Sensor<SensorValue<f64>>,
    pub ambient_temperature: Sensor<SensorValue<f64>>,
    pub ambient_humidity: Sensor<SensorValue<f64>>,
    pub settings: HumiditySettings,
}

#[derive(Clone, Debug)]
pub struct HumiditySettings {
    pub clear_hold: Duration,
    pub minimum_run: Duration,
    pub maximum_run: Duration,
    pub poll_interval: Duration,
    pub absolute_humidity_start_excess: f64,
    pub absolute_humidity_clear_excess: f64,
    pub relative_humidity_start_excess: f64,
    pub relative_humidity_clear_excess: f64,
    pub relative_humidity_start: f64,
    pub relative_humidity_clear: f64,
    pub relative_humidity_extreme: f64,
    pub rise_window: Duration,
    pub absolute_humidity_rise_start: f64,
    pub relative_humidity_rise_start: f64,
}

impl Default for HumiditySettings {
    fn default() -> Self {
        Self {
            clear_hold: Duration::from_secs(5 * 60),
            minimum_run: Duration::from_secs(10 * 60),
            maximum_run: Duration::from_secs(90 * 60),
            poll_interval: Duration::from_secs(30),
            absolute_humidity_start_excess: 2.0,
            absolute_humidity_clear_excess: 0.8,
            relative_humidity_start_excess: 12.0,
            relative_humidity_clear_excess: 6.0,
            relative_humidity_start: 70.0,
            relative_humidity_clear: 65.0,
            relative_humidity_extreme: 80.0,
            rise_window: Duration::from_secs(5 * 60),
            absolute_humidity_rise_start: 0.6,
            relative_humidity_rise_start: 6.0,
        }
    }
}

pub struct HumidityStatus {
    config: HumidityStatusConfig,
    state: PublishedHumidityState,
    humid_started_at: Option<Instant>,
    clear_since: Option<Instant>,
    samples: VecDeque<HumiditySample>,
}

impl HumidityStatus {
    pub fn new(config: HumidityStatusConfig) -> Self {
        Self {
            config,
            state: PublishedHumidityState::Unknown,
            humid_started_at: None,
            clear_since: None,
            samples: VecDeque::new(),
        }
    }

    pub async fn run(mut self, ctx: Context) -> hauto::Result<()> {
        loop {
            let now = Instant::now();
            let evaluation = read_inputs(&ctx, &self.config).await?;
            let next = self.evaluate(now, &evaluation);

            if next.state != self.state {
                println!(
                    "{} humidity: {:?} -> {:?}",
                    self.config.name, self.state, next.state
                );
                self.state = next.state;
            }

            publish_status(&ctx, &self.config, &next).await?;
            self.wait_for_change_or_tick(&ctx).await?;
        }
    }

    fn evaluate(&mut self, now: Instant, evaluation: &HumidityEvaluation) -> HumidityPublish {
        let Some(reading) = evaluation.reading else {
            self.humid_started_at = None;
            self.clear_since = None;
            return HumidityPublish::unknown(evaluation.reason);
        };

        self.record_sample(now, reading);

        let rise_started = humidity_rise_started(&self.samples, now, &self.config.settings);
        let start = reading.humidity_start(&self.config.settings) || rise_started;
        let clear = reading.humidity_clear(&self.config.settings);
        let extreme = reading.extremely_humid(&self.config.settings);

        match self.state {
            PublishedHumidityState::Normal | PublishedHumidityState::Unknown => {
                if start {
                    self.humid_started_at = Some(now);
                    self.clear_since = None;
                    HumidityPublish::humid(
                        reading,
                        start_reason(reading, rise_started, &self.config.settings),
                    )
                } else {
                    self.humid_started_at = None;
                    self.clear_since = None;
                    HumidityPublish::normal(reading, "below_start_thresholds")
                }
            }
            PublishedHumidityState::Humid => {
                let started_at = self.humid_started_at.unwrap_or(now);
                self.humid_started_at = Some(started_at);

                self.clear_since = if clear {
                    Some(self.clear_since.unwrap_or(now))
                } else {
                    None
                };

                let minimum_elapsed =
                    now.duration_since(started_at) >= self.config.settings.minimum_run;
                let clear_held = self.clear_since.is_some_and(|clear_since| {
                    now.duration_since(clear_since) >= self.config.settings.clear_hold
                });
                let maximum_elapsed =
                    now.duration_since(started_at) >= self.config.settings.maximum_run;

                if (minimum_elapsed && clear_held) || (maximum_elapsed && !extreme) {
                    self.humid_started_at = None;
                    self.clear_since = None;
                    HumidityPublish::normal(
                        reading,
                        if maximum_elapsed {
                            "maximum_run_elapsed"
                        } else {
                            "clear_condition_held"
                        },
                    )
                } else {
                    HumidityPublish::humid(
                        reading,
                        if clear {
                            "waiting_for_clear_hold_or_minimum_run"
                        } else {
                            "still_humid"
                        },
                    )
                }
            }
        }
    }

    fn record_sample(&mut self, now: Instant, reading: HumidityReading) {
        self.samples.push_back(HumiditySample {
            at: now,
            absolute_humidity: reading.bathroom_absolute_humidity,
            relative_humidity: Some(reading.bathroom_relative_humidity),
        });

        while self.samples.front().is_some_and(|sample| {
            now.duration_since(sample.at) > self.config.settings.rise_window * 2
        }) {
            self.samples.pop_front();
        }
    }

    async fn wait_for_change_or_tick(&self, ctx: &Context) -> hauto::Result<()> {
        tokio::select! {
            result = self.config.bathroom_temperature.next_change(ctx) => ignore_sensor_change(result),
            result = self.config.bathroom_humidity.next_change(ctx) => ignore_sensor_change(result),
            result = self.config.ambient_temperature.next_change(ctx) => ignore_sensor_change(result),
            result = self.config.ambient_humidity.next_change(ctx) => ignore_sensor_change(result),
            result = ctx.sleep(self.config.settings.poll_interval) => result,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishedHumidityState {
    Normal,
    Humid,
    Unknown,
}

impl PublishedHumidityState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Humid => "humid",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy)]
struct HumiditySample {
    at: Instant,
    absolute_humidity: Option<f64>,
    relative_humidity: Option<f64>,
}

#[derive(Clone, Copy)]
struct HumidityReading {
    bathroom_relative_humidity: f64,
    ambient_relative_humidity: f64,
    bathroom_absolute_humidity: Option<f64>,
    ambient_absolute_humidity: Option<f64>,
}

impl HumidityReading {
    fn humidity_start(self, settings: &HumiditySettings) -> bool {
        self.absolute_humidity_excess()
            .is_some_and(|excess| excess > settings.absolute_humidity_start_excess)
            || self
                .relative_humidity_excess()
                .is_some_and(|excess| excess > settings.relative_humidity_start_excess)
            || self.bathroom_relative_humidity > settings.relative_humidity_start
    }

    fn humidity_clear(self, settings: &HumiditySettings) -> bool {
        self.absolute_humidity_excess()
            .is_some_and(|excess| excess < settings.absolute_humidity_clear_excess)
            && self
                .relative_humidity_excess()
                .is_some_and(|excess| excess < settings.relative_humidity_clear_excess)
            && self.bathroom_relative_humidity < settings.relative_humidity_clear
    }

    fn extremely_humid(self, settings: &HumiditySettings) -> bool {
        self.bathroom_relative_humidity > settings.relative_humidity_extreme
    }

    fn absolute_humidity_excess(self) -> Option<f64> {
        Some(self.bathroom_absolute_humidity? - self.ambient_absolute_humidity?)
    }

    fn relative_humidity_excess(self) -> Option<f64> {
        Some(self.bathroom_relative_humidity - self.ambient_relative_humidity)
    }
}

struct HumidityEvaluation {
    reading: Option<HumidityReading>,
    reason: &'static str,
}

struct HumidityPublish {
    state: PublishedHumidityState,
    reason: &'static str,
    reading: Option<HumidityReading>,
}

impl HumidityPublish {
    fn normal(reading: HumidityReading, reason: &'static str) -> Self {
        Self {
            state: PublishedHumidityState::Normal,
            reason,
            reading: Some(reading),
        }
    }

    fn humid(reading: HumidityReading, reason: &'static str) -> Self {
        Self {
            state: PublishedHumidityState::Humid,
            reason,
            reading: Some(reading),
        }
    }

    fn unknown(reason: &'static str) -> Self {
        Self {
            state: PublishedHumidityState::Unknown,
            reason,
            reading: None,
        }
    }
}

async fn read_inputs(
    ctx: &Context,
    config: &HumidityStatusConfig,
) -> hauto::Result<HumidityEvaluation> {
    let bathroom_temperature = get_optional_sensor(ctx, &config.bathroom_temperature).await?;
    let bathroom_relative_humidity = get_optional_sensor(ctx, &config.bathroom_humidity).await?;
    let ambient_temperature = get_optional_sensor(ctx, &config.ambient_temperature).await?;
    let ambient_relative_humidity = get_optional_sensor(ctx, &config.ambient_humidity).await?;

    let Some(bathroom_relative_humidity) = bathroom_relative_humidity else {
        return Ok(HumidityEvaluation {
            reading: None,
            reason: "bathroom_humidity_unavailable",
        });
    };
    let Some(ambient_relative_humidity) = ambient_relative_humidity else {
        return Ok(HumidityEvaluation {
            reading: None,
            reason: "ambient_humidity_unavailable",
        });
    };

    Ok(HumidityEvaluation {
        reading: Some(HumidityReading {
            bathroom_relative_humidity,
            ambient_relative_humidity,
            bathroom_absolute_humidity: absolute_humidity(
                bathroom_temperature,
                Some(bathroom_relative_humidity),
            ),
            ambient_absolute_humidity: absolute_humidity(
                ambient_temperature,
                Some(ambient_relative_humidity),
            ),
        }),
        reason: "read",
    })
}

async fn get_optional_sensor(
    ctx: &Context,
    sensor: &Sensor<SensorValue<f64>>,
) -> hauto::Result<Option<f64>> {
    match sensor.get(ctx).await {
        Ok(value) => Ok(value.into_value()),
        Err(HautoError::EntityNotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn publish_status(
    ctx: &Context,
    config: &HumidityStatusConfig,
    publish: &HumidityPublish,
) -> hauto::Result<()> {
    let attributes = match publish.reading {
        Some(reading) => json!({
            "friendly_name": format!("{} Excess Humidity", config.name),
            "icon": if publish.state == PublishedHumidityState::Humid {
                "mdi:water-percent-alert"
            } else {
                "mdi:water-percent"
            },
            "reason": publish.reason,
            "bathroom_relative_humidity": reading.bathroom_relative_humidity,
            "ambient_relative_humidity": reading.ambient_relative_humidity,
            "bathroom_absolute_humidity": reading.bathroom_absolute_humidity,
            "ambient_absolute_humidity": reading.ambient_absolute_humidity,
            "absolute_humidity_excess": reading.absolute_humidity_excess(),
            "relative_humidity_excess": reading.relative_humidity_excess(),
        }),
        None => json!({
            "friendly_name": format!("{} Excess Humidity", config.name),
            "icon": "mdi:water-percent-alert",
            "reason": publish.reason,
        }),
    };

    ctx.home_assistant()
        .set_state_raw(
            &config.status_entity,
            StateWrite::new(publish.state.as_str(), attributes)?,
        )
        .await?;
    Ok(())
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
    settings: &HumiditySettings,
) -> bool {
    let Some(current) = samples.back() else {
        return false;
    };

    samples
        .iter()
        .filter(|sample| now.duration_since(sample.at) >= settings.rise_window)
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

fn start_reason(
    reading: HumidityReading,
    rise_started: bool,
    settings: &HumiditySettings,
) -> &'static str {
    if reading
        .absolute_humidity_excess()
        .is_some_and(|excess| excess > settings.absolute_humidity_start_excess)
    {
        "absolute_humidity_excess"
    } else if reading
        .relative_humidity_excess()
        .is_some_and(|excess| excess > settings.relative_humidity_start_excess)
    {
        "relative_humidity_excess"
    } else if reading.bathroom_relative_humidity > settings.relative_humidity_start {
        "relative_humidity"
    } else if rise_started {
        "rate_of_rise"
    } else {
        "unknown"
    }
}

fn ignore_sensor_change(result: hauto::Result<SensorValue<f64>>) -> hauto::Result<()> {
    match result {
        Ok(_) | Err(HautoError::EntityNotFound(_)) => Ok(()),
        Err(error) => Err(error),
    }
}
