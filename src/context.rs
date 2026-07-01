use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    Error, Result, TimerCompletionGuard, TimerControl,
    client::{HomeAssistantClient, StateChangeStream},
    discovery::EntityCatalog,
    entity::{BinarySensor, EntityId, Light},
    runtime::{TaskHandle, TimerHandle},
    state::StateCache,
    wait::{GlobalStateWait, TimeoutResult},
    wait_cancelled,
};

/// Access to Home Assistant and cancellation-aware work for one connection generation.
///
/// Cloning a context does not create a new generation. All clones share the
/// same client, state cache, discovery cache, event sources, and cancellation
/// signal. Once the connection is lost, the generation remains cancelled and
/// its cancellation-aware operations return [`Error::Cancelled`];
/// [`App`](crate::App) supplies automations with a new context after
/// reconnecting.
///
/// [`Context::default`] creates an isolated, empty generation without REST or
/// WebSocket transports. Normal automation code receives a connected context
/// from [`App`](crate::App).
#[derive(Clone, Debug)]
pub struct Context {
    pub(crate) home_assistant: HomeAssistantClient,
}

impl Default for Context {
    fn default() -> Self {
        Self::new_generation()
    }
}

impl Context {
    pub(crate) fn new_generation() -> Self {
        Self {
            home_assistant: HomeAssistantClient::new_generation(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_generation_with_rest_states(
        rest_base_url: impl AsRef<str>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            home_assistant: HomeAssistantClient::new_generation_with_rest_states(
                rest_base_url,
                access_token,
            )?,
        })
    }

    pub(crate) async fn new_generation_with_websocket_and_rest_states(
        rest_base_url: impl AsRef<str>,
        websocket_url: impl AsRef<str>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            home_assistant: HomeAssistantClient::new_generation_with_websocket_and_rest_states(
                rest_base_url,
                websocket_url,
                access_token,
            )
            .await?,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_seeded_states(
        states: impl IntoIterator<Item = crate::state::EntityState>,
    ) -> Self {
        Self {
            home_assistant: HomeAssistantClient::with_seeded_states(states),
        }
    }

    pub(crate) fn cancel_generation(&self) {
        self.home_assistant.cancel_generation();
    }

    /// Returns a low-level client scoped to this context's generation.
    ///
    /// The returned clone shares the generation's state and cancellation
    /// signal. It becomes stale when that generation is cancelled; it does not
    /// automatically follow [`App`](crate::App) to a later connection.
    pub fn home_assistant(&self) -> HomeAssistantClient {
        self.home_assistant.clone()
    }

    /// Returns the entity catalog for this connection generation.
    ///
    /// The first call loads areas and enabled entity-registry entries from Home
    /// Assistant. The resulting snapshot is cached and shared by subsequent
    /// calls in the same generation; a reconnect starts with an empty catalog
    /// cache.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if the generation is cancelled while
    /// loading. Transport, protocol, and malformed catalog response errors are
    /// propagated from Home Assistant.
    pub async fn entity_catalog(&self) -> Result<EntityCatalog> {
        EntityCatalog::load(self.home_assistant.clone()).await
    }

    /// Returns a future that completes when this generation is cancelled.
    ///
    /// The future also completes immediately if cancellation happened before
    /// it was created. It does not report cancellation of later generations.
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + 'static {
        let mut cancelled = self.home_assistant.cancelled_receiver();
        async move {
            if *cancelled.borrow() {
                return;
            }

            while cancelled.changed().await.is_ok() {
                if *cancelled.borrow() {
                    return;
                }
            }
        }
    }

    /// Spawns cancellation-aware work in this connection generation.
    ///
    /// The task returns the future's result if it finishes first. Generation
    /// cancellation drops the supplied future and makes the task return
    /// [`Error::Cancelled`]. Dropping the returned [`TaskHandle`] detaches the
    /// task rather than aborting it; generation cancellation still applies.
    pub fn spawn<F, T>(&self, _future: F) -> TaskHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let mut cancelled = self.home_assistant.cancelled_receiver();
        let handle = tokio::spawn(async move {
            if *cancelled.borrow() {
                return Err(Error::Cancelled);
            }

            tokio::select! {
                result = _future => result,
                _ = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
            }
        });
        TaskHandle::from_join_handle(handle)
    }

    /// Sleeps for `duration`, unless this generation is cancelled first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if the generation is already cancelled or
    /// becomes cancelled before the sleep completes.
    pub async fn sleep(&self, duration: Duration) -> Result<()> {
        self.home_assistant.ensure_generation_active()?;
        let mut cancelled = self.home_assistant.cancelled_receiver();
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
        }
    }

    /// Runs `future` until it completes, `duration` elapses, or the generation is cancelled.
    ///
    /// The future is dropped when the timeout or cancellation wins. Its
    /// successful result is wrapped in [`TimeoutResult::Completed`], while
    /// expiration produces [`TimeoutResult::TimedOut`].
    ///
    /// # Errors
    ///
    /// Propagates the future's error if it completes first. Returns
    /// [`Error::Cancelled`] if the generation is already cancelled or becomes
    /// cancelled first.
    pub async fn timeout<F, T>(&self, duration: Duration, future: F) -> Result<TimeoutResult<T>>
    where
        F: Future<Output = Result<T>> + Send,
        T: Send,
    {
        self.home_assistant.ensure_generation_active()?;
        let mut cancelled = self.home_assistant.cancelled_receiver();
        tokio::select! {
            result = future => result.map(TimeoutResult::Completed),
            () = tokio::time::sleep(duration) => Ok(TimeoutResult::TimedOut),
            () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
        }
    }

    /// Schedules cancellation-aware work to start after `duration`.
    ///
    /// The future is not polled before the delay completes. Cancelling the
    /// returned [`TimerHandle`] or cancelling the generation drops the future
    /// and makes the timer task return [`Error::Cancelled`]. Dropping the handle
    /// alone detaches the timer; it does not cancel the delay or future.
    pub fn run_after<F, T>(&self, duration: Duration, future: F) -> TimerHandle<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let timer = Arc::new(TimerControl::new());
        let timer_for_task = timer.clone();
        let completion_for_task = timer.clone();
        let mut timer_cancelled = timer.subscribe();
        let mut cancelled = self.home_assistant.cancelled_receiver();
        let handle = tokio::spawn(async move {
            let _completion = TimerCompletionGuard(completion_for_task);
            async move {
                if *cancelled.borrow() {
                    return Err(Error::Cancelled);
                }

                tokio::select! {
                    () = tokio::time::sleep(duration) => {}
                    () = wait_cancelled(&mut timer_cancelled) => return Err(Error::Cancelled),
                    () = wait_cancelled(&mut cancelled) => return Err(Error::Cancelled),
                }

                if timer_for_task.is_cancelled() {
                    return Err(Error::Cancelled);
                }

                tokio::select! {
                    result = future => result,
                    () = wait_cancelled(&mut timer_cancelled) => Err(Error::Cancelled),
                    () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
                }
            }
            .await
        });
        TimerHandle::from_join_handle(handle, timer)
    }

    /// Subscribes to later cached state changes for `entity` in this generation.
    ///
    /// The stream does not emit the current cached state. It is tied to this
    /// generation and does not continue across reconnects. If its bounded
    /// buffer is overrun it reports a terminal
    /// [`EventStreamError::Lagged`](crate::client::EventStreamError::Lagged);
    /// it ends when the generation's event source closes.
    ///
    /// Creating this stream does not check whether the generation is already
    /// cancelled, and cancellation is not emitted as a stream item. Code that
    /// must stop promptly on connection loss should also await
    /// [`Context::cancelled`].
    pub fn state_changes(&self, entity: &EntityId) -> StateChangeStream {
        StateChangeStream::new(
            self.home_assistant.generation.state_changes.subscribe(),
            Some(entity.clone()),
        )
    }

    /// Builds a wait over the complete cached state snapshot.
    ///
    /// The predicate is evaluated immediately and again after any entity state
    /// change. Predicate errors are propagated. The returned builder can
    /// require continuous satisfaction or impose a timeout.
    ///
    /// Because every state change can invoke the predicate, it should remain
    /// synchronous and inexpensive. Prefer an entity-specific wait when the
    /// condition depends on only one entity.
    ///
    /// Awaiting the wait returns [`Error::Cancelled`] if this generation is
    /// cancelled; a held interval is not resumed in a later generation.
    pub fn wait_until_state<F>(&self, predicate: F) -> GlobalStateWait<'_, F>
    where
        F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
    {
        GlobalStateWait::new(self, predicate)
    }

    /// Subscribes to later state changes for `sensor` in this generation.
    ///
    /// This is the typed convenience form of [`Context::state_changes`] and has
    /// the same lag, closure, and reconnect behavior.
    pub fn binary_sensor_changes(&self, sensor: &BinarySensor) -> StateChangeStream {
        self.state_changes(sensor.entity_id())
    }

    /// Subscribes to later state changes for `light` in this generation.
    ///
    /// This is the typed convenience form of [`Context::state_changes`] and has
    /// the same lag, closure, and reconnect behavior.
    pub fn light_changes(&self, light: &Light) -> StateChangeStream {
        self.state_changes(light.entity_id())
    }
}
