//! Shared fan controller that consumes derived humidity status sensors.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hauto::{BinarySensor, BinaryState, Context, Error as HautoError, Sensor, Switch};

#[derive(Clone, Debug)]
pub struct FanControlConfig {
    pub fan: Switch,
    pub bathrooms: [FanBathroomConfig; 2],
    pub settings: FanSettings,
}

#[derive(Clone, Debug)]
pub struct FanBathroomConfig {
    pub name: String,
    pub humidity_status: Sensor<String>,
    pub occupancy: BinarySensor,
}

#[derive(Clone, Debug)]
pub struct FanSettings {
    pub quiet_hours: QuietHours,
    pub occupancy_post_run: Duration,
    pub minimum_on_time: Duration,
    pub minimum_off_time: Duration,
    pub poll_interval: Duration,
}

impl Default for FanSettings {
    fn default() -> Self {
        Self {
            quiet_hours: QuietHours {
                start_minute: 0,
                end_minute: 8 * 60,
                utc_offset_minutes: 0,
            },
            occupancy_post_run: Duration::from_secs(7 * 60),
            minimum_on_time: Duration::from_secs(2 * 60),
            minimum_off_time: Duration::from_secs(60),
            poll_interval: Duration::from_secs(30),
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

pub struct FanControl {
    config: FanControlConfig,
    bathrooms: [BathroomOccupancyRuntime; 2],
    fan_on: bool,
    last_on: Option<Instant>,
    last_off: Option<Instant>,
}

impl FanControl {
    pub fn new(config: FanControlConfig) -> Self {
        Self {
            config,
            bathrooms: [
                BathroomOccupancyRuntime::default(),
                BathroomOccupancyRuntime::default(),
            ],
            fan_on: false,
            last_on: None,
            last_off: None,
        }
    }

    pub async fn run(mut self, ctx: Context) -> hauto::Result<()> {
        loop {
            let now = Instant::now();
            let quiet = self.config.settings.quiet_hours.is_quiet_now();
            let mut humidity_demand = false;

            for index in 0..self.bathrooms.len() {
                let bathroom = &self.config.bathrooms[index];
                let humid = is_humid(&ctx, bathroom).await?;
                let occupied = is_occupied(&ctx, bathroom).await?;
                humidity_demand |= humid;

                let previous_state = self.bathrooms[index].state;
                self.bathrooms[index].advance(
                    now,
                    quiet,
                    occupied,
                    self.config.settings.occupancy_post_run,
                );
                if self.bathrooms[index].state != previous_state {
                    println!(
                        "{} occupancy: {:?} -> {:?}",
                        bathroom.name, previous_state, self.bathrooms[index].state
                    );
                }
            }

            self.apply_fan(&ctx, now, quiet, humidity_demand).await?;
            self.wait_for_change_or_tick(&ctx).await?;
        }
    }

    async fn apply_fan(
        &mut self,
        ctx: &Context,
        now: Instant,
        quiet: bool,
        humidity_demand: bool,
    ) -> hauto::Result<()> {
        let occupancy_demand = self
            .bathrooms
            .iter()
            .any(|bathroom| !matches!(bathroom.state, OccupancyState::Idle));
        let desired_on = humidity_demand || (!quiet && occupancy_demand);
        let force_quiet_occupancy_off = quiet && !humidity_demand;

        if desired_on && !self.fan_on {
            if self.last_off.is_some_and(|last| {
                now.duration_since(last) < self.config.settings.minimum_off_time
            }) {
                return Ok(());
            }

            self.config.fan.turn_on(ctx).await?;
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

            self.config.fan.turn_off(ctx).await?;
            self.fan_on = false;
            self.last_off = Some(now);
        }

        Ok(())
    }

    async fn wait_for_change_or_tick(&self, ctx: &Context) -> hauto::Result<()> {
        let bathroom_1 = &self.config.bathrooms[0];
        let bathroom_2 = &self.config.bathrooms[1];

        tokio::select! {
            result = bathroom_1.humidity_status.next_change(ctx) => ignore_string_change(result),
            result = bathroom_1.occupancy.next_change(ctx) => ignore_binary_change(result),
            result = bathroom_2.humidity_status.next_change(ctx) => ignore_string_change(result),
            result = bathroom_2.occupancy.next_change(ctx) => ignore_binary_change(result),
            result = ctx.sleep(self.config.settings.poll_interval) => result,
        }
    }
}

#[derive(Default)]
struct BathroomOccupancyRuntime {
    state: OccupancyState,
}

impl BathroomOccupancyRuntime {
    fn advance(&mut self, now: Instant, quiet: bool, occupied: bool, post_run: Duration) {
        self.state = match self.state {
            OccupancyState::Idle => {
                if occupied && !quiet {
                    OccupancyState::Occupied
                } else {
                    OccupancyState::Idle
                }
            }
            OccupancyState::Occupied => {
                if quiet {
                    OccupancyState::Idle
                } else if occupied {
                    OccupancyState::Occupied
                } else {
                    OccupancyState::Drying {
                        until: now + post_run,
                    }
                }
            }
            OccupancyState::Drying { until } => {
                if quiet {
                    OccupancyState::Idle
                } else if occupied {
                    OccupancyState::Occupied
                } else if now >= until {
                    OccupancyState::Idle
                } else {
                    OccupancyState::Drying { until }
                }
            }
        };
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OccupancyState {
    #[default]
    Idle,
    Occupied,
    Drying {
        until: Instant,
    },
}

async fn is_humid(ctx: &Context, bathroom: &FanBathroomConfig) -> hauto::Result<bool> {
    match bathroom.humidity_status.get(ctx).await {
        Ok(state) => Ok(state == "humid"),
        Err(HautoError::EntityNotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn is_occupied(ctx: &Context, bathroom: &FanBathroomConfig) -> hauto::Result<bool> {
    match bathroom.occupancy.get(ctx).await {
        Ok(BinaryState::On) => Ok(true),
        Ok(BinaryState::Off | BinaryState::Unknown | BinaryState::Unavailable) => Ok(false),
        Err(HautoError::EntityNotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn ignore_string_change(result: hauto::Result<String>) -> hauto::Result<()> {
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
