//! Drive one shared operation even if an individual caller stops waiting.
use super::*;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
struct Flight<T: Clone> {
    id: uuid::Uuid,
    receiver: watch::Receiver<Option<Result<T>>>,
}
pub(super) struct SingleFlight<T: Clone>(Arc<Mutex<Option<Flight<T>>>>);
impl<T: Clone> Default for SingleFlight<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}
impl<T: Clone + Send + Sync + 'static> SingleFlight<T> {
    pub async fn run(&self, future: impl Future<Output = Result<T>> + Send + 'static) -> Result<T> {
        let mut receiver = {
            let mut state = self.0.lock().unwrap();
            if let Some(flight) = state.as_ref() {
                flight.receiver.clone()
            } else {
                let id = uuid::Uuid::new_v4();
                let (tx, rx) = watch::channel(None);
                *state = Some(Flight {
                    id,
                    receiver: rx.clone(),
                });
                let state = self.0.clone();
                tokio::spawn(async move {
                    struct Clear<T: Clone> {
                        state: Arc<Mutex<Option<Flight<T>>>>,
                        id: uuid::Uuid,
                    }
                    impl<T: Clone> Drop for Clear<T> {
                        fn drop(&mut self) {
                            let mut s = self.state.lock().unwrap();
                            if s.as_ref().is_some_and(|f| f.id == self.id) {
                                *s = None;
                            }
                        }
                    }
                    let _clear = Clear { state, id };
                    tx.send_replace(Some(future.await));
                });
                rx
            }
        };
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| unavailable("Credential refresh was interrupted."))?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{Notify, oneshot};
    #[tokio::test]
    async fn cancelled_waiter_keeps_shared_work_driven_and_failure_is_not_cached() {
        let flight = Arc::new(SingleFlight::<u32>::default());
        let release = Arc::new(Notify::new());
        let (started, ready) = oneshot::channel();
        let first = {
            let flight = flight.clone();
            let release = release.clone();
            tokio::spawn(async move {
                flight
                    .run(async move {
                        started.send(()).unwrap();
                        release.notified().await;
                        Ok(7)
                    })
                    .await
            })
        };
        ready.await.unwrap();
        first.abort();
        let _ = first.await;
        let second = {
            let flight = flight.clone();
            tokio::spawn(async move {
                flight
                    .run(async { panic!("A pending flight must be shared") })
                    .await
            })
        };
        tokio::task::yield_now().await;
        release.notify_one();
        assert_eq!(second.await.unwrap().unwrap(), 7);
        tokio::task::yield_now().await;
        assert!(
            flight
                .run(async { Err(unavailable("Fixture failure")) })
                .await
                .is_err()
        );
        tokio::task::yield_now().await;
        assert_eq!(flight.run(async { Ok(9) }).await.unwrap(), 9);
    }
    #[tokio::test]
    async fn simultaneous_waiters_share_the_same_failed_operation() {
        let flight = Arc::new(SingleFlight::<u32>::default());
        let release = Arc::new(Notify::new());
        let (started, ready) = oneshot::channel();
        let first = {
            let flight = flight.clone();
            let release = release.clone();
            tokio::spawn(async move {
                flight
                    .run(async move {
                        started.send(()).unwrap();
                        release.notified().await;
                        Err(unavailable("Shared failure"))
                    })
                    .await
            })
        };
        ready.await.unwrap();
        let mut jobs = vec![];
        for _ in 0..8 {
            let flight = flight.clone();
            jobs.push(tokio::spawn(async move {
                flight.run(async { panic!("Duplicate refresh") }).await
            }));
        }
        tokio::task::yield_now().await;
        release.notify_one();
        assert_eq!(
            first.await.unwrap().unwrap_err().to_string(),
            "Shared failure"
        );
        for job in jobs {
            assert_eq!(
                job.await.unwrap().unwrap_err().to_string(),
                "Shared failure"
            );
        }
    }
}
