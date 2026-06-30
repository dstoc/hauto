//! A small Rust automation framework scaffold for Home Assistant.
//!
//! This crate currently defines the public surface proposed for `hauto`.
//! Runtime, transport, cache, and event fan-out behavior are intentionally
//! placeholder implementations for now.

use std::{future::Future, pin::Pin};

mod app;
mod client;
mod context;
mod entity;
mod error;
mod rest;
mod services;
mod state;
mod streams;
#[cfg(test)]
mod tests;
mod timer;
mod wait;

pub use app::{App, Automation};
pub use client::HomeAssistantClient;
pub use context::Context;
pub use entity::{BinarySensor, EntityId, Light, Sensor, Switch};
pub use error::Error;
#[cfg(test)]
pub(crate) use rest::{RestStateError, RestStateMethod, RestStateResponse};
pub(crate) use rest::{
    RestStateRequest, RestStateTransport, map_delete_state_response, map_set_state_response,
};
pub use services::{LightTurnOff, LightTurnOn};
pub(crate) use services::{service_entity, validate_domain_service};
pub use state::{
    Availability, BinaryState, DeleteStateResult, EntityState, SetStateResult, StateChangedEvent,
    StateWrite,
};
pub use streams::{EventStreamError, RawEventStream, StateChangeStream};
pub use timer::{TaskHandle, TimerHandle};
pub(crate) use timer::{TimerCompletionGuard, TimerControl, wait_cancelled};
pub use wait::{
    HoldResult, StateExpectation, StateWait, TimedStateWait, TimeoutResult, WaitResult,
};

pub type Result<T, E = Error> = std::result::Result<T, E>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
