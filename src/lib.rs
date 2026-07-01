//! Async Rust automation primitives for Home Assistant.
//!
//! `hauto` provides typed entity handles, cancellation-aware timers, state-change
//! streams, typed wait builders, service calls, REST state publishing/deletion,
//! and a WebSocket-backed [`App`] runtime.
//!
//! The usual entrypoint is [`App`]. It connects to Home Assistant, fetches the
//! initial state snapshot, subscribes to `state_changed` events, and runs each
//! registered automation with a cloneable [`Context`]. If the Home Assistant
//! connection generation is replaced or lost, the current context is cancelled
//! and automations are restarted by [`App::run`].
//!
//! # Basic automation
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use hauto::{App, BinarySensor, HoldResult, Light, LightTurnOff, LightTurnOn};
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> hauto::Result<()> {
//!     let occupancy = BinarySensor::new("binary_sensor.office_occupancy")?;
//!     let light = Light::new("light.office")?;
//!
//!     App::new("http://homeassistant.local:8123", "long-lived-access-token")
//!         .automation_fn("office occupancy light", move |ctx| {
//!             let occupancy = occupancy.clone();
//!             let light = light.clone();
//!
//!             async move {
//!                 loop {
//!                     occupancy.wait_until_on(&ctx).await?;
//!                     light.turn_on(&ctx, LightTurnOn::default()).await?;
//!
//!                     if matches!(
//!                         occupancy
//!                             .expect_off(&ctx)
//!                             .for_at_least(Duration::from_secs(30))
//!                             .await?,
//!                         HoldResult::Held
//!                     ) {
//!                         light.turn_off(&ctx, LightTurnOff::default()).await?;
//!                     }
//!                 }
//!             }
//!         })
//!         .run()
//!         .await
//! }
//! ```
//!
//! # Global state predicates
//!
//! Use [`Context::wait_until_state`] when a condition spans multiple entities
//! and must be true at the same time.
//!
//! ```no_run
//! # use std::time::Duration;
//! # use hauto::{Context, Sensor};
//! # async fn example(ctx: Context) -> hauto::Result<()> {
//! let temperature = Sensor::<f64>::new("sensor.office_temperature")?;
//! let humidity = Sensor::<f64>::new("sensor.office_humidity")?;
//!
//! ctx.wait_until_state(move |state| {
//!     let Some(t) = temperature.read(state)? else {
//!         return Ok(false);
//!     };
//!     let Some(h) = humidity.read(state)? else {
//!         return Ok(false);
//!     };
//!
//!     Ok(t >= 24.0 && h <= 55.0)
//! })
//! .for_at_least(Duration::from_secs(30))
//! .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Typed entity handles validate entity-id domains when constructed, but they do
//! not check whether the entity currently exists in Home Assistant. Existence
//! and state decoding are checked when reading state, waiting for state, or
//! calling Home Assistant.

mod app;
mod cache;
pub mod client;
mod context;
pub mod discovery;
pub mod entity;
mod error;
mod rest;
mod services;
pub mod state;
mod streams;
#[cfg(test)]
mod tests;
mod timer;
pub mod wait;
#[allow(dead_code)]
mod ws;

/// Runtime entrypoints and cancellation-aware task and timer handles.
pub mod runtime {
    use std::{future::Future, pin::Pin};

    pub use crate::{
        app::{App, Automation},
        context::Context,
        timer::{TaskHandle, TimerHandle},
    };

    /// A boxed, sendable future used by automation implementations.
    pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
}

/// Typed Home Assistant service-call options.
pub mod service {
    pub use crate::services::{LightTurnOff, LightTurnOn};
}

pub use entity::{BinarySensor, BinaryState, EntityId, Light, Sensor, SensorValue, Switch};
pub use error::Error;
pub use runtime::{App, Automation, Context};
pub use service::{LightTurnOff, LightTurnOn};
pub use wait::{HoldResult, TimeoutResult, WaitResult};

#[cfg(test)]
pub(crate) use rest::{RestStateError, RestStateMethod, RestStateResponse};
pub(crate) use rest::{
    RestStateRequest, RestStateTransport, map_delete_state_response, map_set_state_response,
};
pub(crate) use services::{service_entity, validate_domain_service};
pub(crate) use timer::{TimerCompletionGuard, TimerControl, wait_cancelled};
pub(crate) use ws::WsTransport;

pub type Result<T, E = Error> = std::result::Result<T, E>;
