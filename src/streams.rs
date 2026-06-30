use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_core::Stream;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

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
    _private: (),
}

impl RawEventStream {
    pub(crate) fn placeholder() -> Self {
        Self { _private: () }
    }
}
