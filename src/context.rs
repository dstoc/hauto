use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    BinarySensor, EntityCatalog, EntityId, Error, GlobalStateWait, HomeAssistantClient, Light,
    Result, StateCache, StateChangeStream, TaskHandle, TimeoutResult, TimerCompletionGuard,
    TimerControl, TimerHandle, wait_cancelled,
};

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
    pub(crate) fn with_seeded_states(states: impl IntoIterator<Item = crate::EntityState>) -> Self {
        Self {
            home_assistant: HomeAssistantClient::with_seeded_states(states),
        }
    }

    pub(crate) fn cancel_generation(&self) {
        self.home_assistant.cancel_generation();
    }

    pub fn home_assistant(&self) -> HomeAssistantClient {
        self.home_assistant.clone()
    }

    pub async fn entity_catalog(&self) -> Result<EntityCatalog> {
        EntityCatalog::load(self.home_assistant.clone()).await
    }

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

    pub async fn sleep(&self, duration: Duration) -> Result<()> {
        self.home_assistant.ensure_generation_active()?;
        let mut cancelled = self.home_assistant.cancelled_receiver();
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = wait_cancelled(&mut cancelled) => Err(Error::Cancelled),
        }
    }

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

    pub fn state_changes(&self, entity: &EntityId) -> StateChangeStream {
        StateChangeStream::new(
            self.home_assistant.generation.state_changes.subscribe(),
            Some(entity.clone()),
        )
    }

    pub fn wait_until_state<F>(&self, predicate: F) -> GlobalStateWait<'_, F>
    where
        F: Fn(&StateCache) -> Result<bool> + Send + Sync + 'static,
    {
        GlobalStateWait::new(self, predicate)
    }

    pub fn binary_sensor_changes(&self, sensor: &BinarySensor) -> StateChangeStream {
        self.state_changes(sensor.entity_id())
    }

    pub fn light_changes(&self, light: &Light) -> StateChangeStream {
        self.state_changes(light.entity_id())
    }
}
