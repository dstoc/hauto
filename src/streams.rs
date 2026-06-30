use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use serde_json::Value;

use crate::{EntityId, StateChangedEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStreamError {
    Lagged { dropped: Option<usize> },
    ConnectionLost,
}

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
