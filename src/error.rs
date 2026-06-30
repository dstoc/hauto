use thiserror::Error as ThisError;

use crate::{EntityId, EventStreamError};

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("authentication failed: {0}")]
    Authentication(String),
    #[error("entity not found: {0}")]
    EntityNotFound(EntityId),
    #[error("invalid entity id `{value}`: {reason}")]
    InvalidEntityId { value: String, reason: String },
    #[error("invalid domain for `{entity_id}`: expected `{expected}`, got `{actual}`")]
    InvalidDomain {
        entity_id: EntityId,
        expected: &'static str,
        actual: String,
    },
    #[error("invalid state for `{entity_id}`: {reason}")]
    InvalidState { entity_id: EntityId, reason: String },
    #[error("service call rejected: {0}")]
    ServiceRejected(String),
    #[error("service call was not sent: {0}")]
    NotSent(String),
    #[error("operation outcome is unknown: {0}")]
    OutcomeUnknown(String),
    #[error("automation task failed: {0}")]
    AutomationTask(String),
    #[error("event stream error: {0:?}")]
    EventStream(EventStreamError),
    #[error("context was cancelled")]
    Cancelled,
    #[error("invalid service options: {0}")]
    InvalidServiceOptions(String),
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
}
