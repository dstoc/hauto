//! Generation-scoped event stream implementations re-exported by [`crate::client`].

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use serde_json::Value;

use crate::{entity::EntityId, state::StateChangedEvent};

#[cfg(test)]
mod tests;

/// A terminal failure reported as the last item of an event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStreamError {
    /// The bounded local broadcast buffer overflowed.
    ///
    /// The stream terminates after this item because its event sequence is no
    /// longer complete.
    Lagged {
        /// Number of skipped items when it can be represented as `usize`.
        dropped: Option<usize>,
    },
    /// The stream's underlying Home Assistant connection was lost.
    ///
    /// Current stream implementations normally signal source closure by ending
    /// with `None`; this variant is available where connection loss is surfaced
    /// explicitly.
    ConnectionLost,
}

/// A stream of state-change events received after subscription.
///
/// No current cached state is emitted. A stream created by
/// [`Context::state_changes`](crate::Context::state_changes) uses an exact
/// entity-ID filter; [`HomeAssistantClient::subscribe_state_changes`](crate::client::HomeAssistantClient::subscribe_state_changes)
/// is unfiltered. Non-matching events are skipped.
///
/// Lag produces one terminal [`EventStreamError::Lagged`] item. Closing the
/// generation's event source ends the stream with `None`. Streams do not
/// reconnect or continue into the next runtime generation.
#[derive(Debug)]
pub struct StateChangeStream {
    receiver: BroadcastStream<StateChangedEvent>,
    entity_filter: Option<EntityId>,
    terminal: bool,
}

impl StateChangeStream {
    pub(crate) fn new(
        receiver: broadcast::Receiver<StateChangedEvent>,
        entity_filter: Option<EntityId>,
    ) -> Self {
        Self {
            receiver: BroadcastStream::new(receiver),
            entity_filter,
            terminal: false,
        }
    }

    /// Waits for the next matching event or terminal stream error.
    ///
    /// Returns `None` after source closure or after a terminal lag error has
    /// already been returned.
    pub async fn next(
        &mut self,
    ) -> Option<std::result::Result<StateChangedEvent, EventStreamError>> {
        std::future::poll_fn(|cx| Stream::poll_next(Pin::new(&mut *self), cx)).await
    }
}

impl Stream for StateChangeStream {
    type Item = std::result::Result<StateChangedEvent, EventStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminal {
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut self.receiver).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if self
                        .entity_filter
                        .as_ref()
                        .is_none_or(|entity_id| event.entity_id == *entity_id)
                    {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(dropped)))) => {
                    self.terminal = true;
                    let dropped = usize::try_from(dropped).ok();
                    return Poll::Ready(Some(Err(EventStreamError::Lagged { dropped })));
                }
                Poll::Ready(None) => {
                    self.terminal = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// A stream of raw Home Assistant event objects received after subscription.
///
/// A requested event type is matched exactly and case-sensitively against the
/// string `event_type` field; non-matching or malformed event objects are
/// skipped. There is no replay. Lag is a terminal error, source closure returns
/// `None`, and the stream never reconnects into a later generation.
#[derive(Debug)]
pub struct RawEventStream {
    receiver: Option<BroadcastStream<Value>>,
    event_type_filter: Option<String>,
    terminal: bool,
}

impl RawEventStream {
    pub(crate) fn placeholder() -> Self {
        Self {
            receiver: None,
            event_type_filter: None,
            terminal: true,
        }
    }

    pub(crate) fn new(
        receiver: broadcast::Receiver<Value>,
        event_type_filter: Option<String>,
    ) -> Self {
        Self {
            receiver: Some(BroadcastStream::new(receiver)),
            event_type_filter,
            terminal: false,
        }
    }

    /// Waits for the next matching raw event or terminal stream error.
    ///
    /// Returns `None` after source closure or after a terminal lag error has
    /// already been returned.
    pub async fn next(&mut self) -> Option<std::result::Result<Value, EventStreamError>> {
        std::future::poll_fn(|cx| Stream::poll_next(Pin::new(&mut *self), cx)).await
    }
}

impl Stream for RawEventStream {
    type Item = std::result::Result<Value, EventStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.terminal {
            return Poll::Ready(None);
        }

        loop {
            let Some(receiver) = &mut self.receiver else {
                self.terminal = true;
                return Poll::Ready(None);
            };
            match Pin::new(receiver).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if raw_event_matches(self.event_type_filter.as_deref(), &event) {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(dropped)))) => {
                    self.terminal = true;
                    let dropped = usize::try_from(dropped).ok();
                    return Poll::Ready(Some(Err(EventStreamError::Lagged { dropped })));
                }
                Poll::Ready(None) => {
                    self.terminal = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn raw_event_matches(event_type_filter: Option<&str>, event: &Value) -> bool {
    event_type_filter.is_none_or(|expected| {
        event
            .get("event_type")
            .and_then(Value::as_str)
            .is_some_and(|actual| actual == expected)
    })
}
