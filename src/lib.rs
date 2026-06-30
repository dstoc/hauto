//! A small Rust automation framework scaffold for Home Assistant.
//!
//! This crate currently defines the public surface proposed for `hauto` and
//! includes in-memory state caching, cancellation-aware task/timer helpers,
//! event fan-out primitives, REST state publishing/deletion, and a WebSocket
//! runtime for raw commands, service calls, state snapshots, and state-change
//! subscriptions.

use std::{future::Future, pin::Pin};

mod app;
mod cache;
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
#[allow(dead_code)]
mod ws;

pub use app::{App, Automation};
pub use cache::StateCache;
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
pub(crate) use ws::WsTransport;

pub type Result<T, E = Error> = std::result::Result<T, E>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
