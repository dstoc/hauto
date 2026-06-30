use tokio::sync::broadcast;

use crate::{EntityId, StateChangedEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventStreamError {
    Lagged { dropped: Option<usize> },
    ConnectionLost,
}

#[derive(Debug)]
pub struct StateChangeStream {
    receiver: broadcast::Receiver<StateChangedEvent>,
    entity_filter: Option<EntityId>,
    terminal: bool,
}

impl StateChangeStream {
    pub(crate) fn new(
        receiver: broadcast::Receiver<StateChangedEvent>,
        entity_filter: Option<EntityId>,
    ) -> Self {
        Self {
            receiver,
            entity_filter,
            terminal: false,
        }
    }

    pub async fn next(
        &mut self,
    ) -> Option<std::result::Result<StateChangedEvent, EventStreamError>> {
        if self.terminal {
            return None;
        }

        loop {
            match self.receiver.recv().await {
                Ok(event) => {
                    if self
                        .entity_filter
                        .as_ref()
                        .is_none_or(|entity_id| event.entity_id == *entity_id)
                    {
                        return Some(Ok(event));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    self.terminal = true;
                    let dropped = usize::try_from(dropped).ok();
                    return Some(Err(EventStreamError::Lagged { dropped }));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    self.terminal = true;
                    return None;
                }
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
