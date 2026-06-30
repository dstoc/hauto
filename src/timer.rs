use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
};

use tokio::{sync::watch, task::JoinHandle};

use crate::{BoxFuture, Error, Result};

pub struct TaskHandle<T> {
    inner: BoxFuture<Result<T>>,
}

impl<T: Send + 'static> TaskHandle<T> {
    pub(crate) fn from_join_handle(handle: JoinHandle<Result<T>>) -> Self {
        Self {
            inner: Box::pin(async move {
                handle
                    .await
                    .unwrap_or_else(|error| Err(Error::AutomationTask(error.to_string())))
            }),
        }
    }
}

impl<T> Future for TaskHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

pub struct TimerHandle<T> {
    inner: BoxFuture<Result<T>>,
    control: Arc<TimerControl>,
}

impl<T: Send + 'static> TimerHandle<T> {
    pub(crate) fn from_join_handle(
        handle: JoinHandle<Result<T>>,
        control: Arc<TimerControl>,
    ) -> Self {
        Self {
            inner: Box::pin(async move {
                handle
                    .await
                    .unwrap_or_else(|error| Err(Error::AutomationTask(error.to_string())))
            }),
            control,
        }
    }

    pub async fn cancel(&mut self) -> Result<()> {
        self.control.cancel();
        self.control.wait_complete().await;
        Ok(())
    }
}

impl<T> Future for TimerHandle<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

#[derive(Debug)]
pub(crate) struct TimerCompletionGuard(pub(crate) Arc<TimerControl>);

impl Drop for TimerCompletionGuard {
    fn drop(&mut self) {
        self.0.complete();
    }
}

#[derive(Debug)]
pub(crate) struct TimerControl {
    is_cancelled: AtomicBool,
    cancelled: watch::Sender<bool>,
    complete: watch::Sender<bool>,
}

impl TimerControl {
    pub(crate) fn new() -> Self {
        let (cancelled, _receiver) = watch::channel(false);
        let (complete, _receiver) = watch::channel(false);
        Self {
            is_cancelled: AtomicBool::new(false),
            cancelled,
            complete,
        }
    }

    fn cancel(&self) {
        if !self.is_cancelled.swap(true, Ordering::AcqRel) {
            let _ = self.cancelled.send(true);
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.cancelled.subscribe()
    }

    fn complete(&self) {
        let _ = self.complete.send(true);
    }

    async fn wait_complete(&self) {
        let mut complete = self.complete.subscribe();
        if *complete.borrow() {
            return;
        }

        while complete.changed().await.is_ok() {
            if *complete.borrow() {
                return;
            }
        }
    }
}

pub(crate) async fn wait_cancelled(cancelled: &mut watch::Receiver<bool>) {
    if *cancelled.borrow() {
        return;
    }

    while cancelled.changed().await.is_ok() {
        if *cancelled.borrow() {
            return;
        }
    }
}
