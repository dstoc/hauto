//! Error contracts shared by the public API.

use thiserror::Error as ThisError;

use crate::{client::EventStreamError, discovery::AreaId, entity::EntityId};

/// An error produced by Home Assistant communication, validation, or runtime work.
#[derive(Debug, ThisError)]
pub enum Error {
    /// A connection or malformed successful-response failure with no indicated
    /// side-effect ambiguity. The string describes the transport or decoding
    /// failure.
    #[error("connection error: {0}")]
    Connection(String),
    /// Home Assistant rejected the supplied credentials or authentication flow.
    /// The string contains the protocol diagnostic.
    #[error("authentication failed: {0}")]
    Authentication(String),
    /// The requested entity was absent from the current generation's state
    /// cache; the payload is its entity ID.
    #[error("entity not found: {0}")]
    EntityNotFound(EntityId),
    /// No discovered area matched the requested display name.
    #[error("area not found: {name}")]
    AreaNotFound {
        /// The area name supplied by the caller.
        name: String,
    },
    /// Multiple discovered areas matched the requested display name.
    #[error("area name `{name}` is ambiguous; candidates: {candidates:?}")]
    AreaAmbiguous {
        /// The area name supplied by the caller.
        name: String,
        /// Every matching area identifier.
        candidates: Vec<AreaId>,
    },
    /// A discovery query matched no entity.
    #[error("entity query found no matches: {query}")]
    EntityQueryNotFound {
        /// A human-readable description of the applied filters.
        query: String,
    },
    /// A singular discovery query matched more than one entity.
    #[error("entity query is ambiguous ({query}); candidates: {candidates:?}")]
    EntityQueryAmbiguous {
        /// Applied filters plus candidate metadata for diagnosis.
        query: String,
        /// Every matching entity identifier.
        candidates: Vec<EntityId>,
    },
    /// An entity-ID string violated Home Assistant entity-ID syntax.
    #[error("invalid entity id `{value}`: {reason}")]
    InvalidEntityId {
        /// The rejected input string.
        value: String,
        /// The specific syntax violation.
        reason: String,
    },
    /// An entity ID did not belong to the domain required by a typed operation.
    #[error("invalid domain for `{entity_id}`: expected `{expected}`, got `{actual}`")]
    InvalidDomain {
        /// The validated entity ID with the wrong domain.
        entity_id: EntityId,
        /// The domain required by the typed API.
        expected: &'static str,
        /// The domain found in `entity_id`.
        actual: String,
    },
    /// A raw Home Assistant state could not be decoded under the requested
    /// typed state policy.
    #[error("invalid state for `{entity_id}`: {reason}")]
    InvalidState {
        /// The entity whose state failed decoding.
        entity_id: EntityId,
        /// The decoding or validation failure.
        reason: String,
    },
    /// Home Assistant definitively rejected a command or returned an
    /// unsupported REST status. The string contains the rejection diagnostic.
    #[error("service call rejected: {0}")]
    ServiceRejected(String),
    /// A command was definitively not sent, normally because its socket was
    /// already known to be closed. The string explains the pre-send failure.
    #[error("service call was not sent: {0}")]
    NotSent(String),
    /// Sending was attempted but no definitive result established whether the
    /// operation took effect; callers should not blindly retry. The string
    /// describes where certainty was lost.
    #[error("operation outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// An automation returned an error, or a spawned automation/helper task
    /// panicked, was aborted, or otherwise failed to join. The string identifies
    /// the task and its failure.
    #[error("automation task failed: {0}")]
    AutomationTask(String),
    /// A state/event consumer encountered the enclosed terminal stream failure.
    #[error("event stream error: {0:?}")]
    EventStream(EventStreamError),
    /// The operation belonged to a cancelled connection generation, context,
    /// task, or timer.
    #[error("context was cancelled")]
    Cancelled,
    /// Service options or a raw command failed local validation before sending;
    /// the string describes the invalid shape or value.
    #[error("invalid service options: {0}")]
    InvalidServiceOptions(String),
    /// The requested operation is unavailable on the configured client
    /// transport; the static string identifies that operation.
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),
}
