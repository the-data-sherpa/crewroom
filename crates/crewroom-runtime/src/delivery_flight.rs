//! One driven provider operation, shared by waiters and drained by shutdown.
use crate::conflict;
use crewroom_core::Result;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
type Reply = watch::Receiver<Option<std::result::Result<(), String>>>;
#[derive(Default)]
struct State {
    closed: bool,
    active: Option<(uuid::Uuid, Reply)>,
}
#[derive(Clone, Default)]
pub(crate) struct DeliveryFlight(Arc<Mutex<State>>);
impl DeliveryFlight {
    pub fn with_status<T>(&self, operation: impl FnOnce(bool) -> Result<T>) -> Result<T> {
        let state = self.0.lock().unwrap();
        operation(state.closed)
    }
    /// The callback must be synchronous; the lock order is flight, then store.
    pub fn with_open<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let state = self.0.lock().unwrap();
        if state.closed {
            return Err(conflict("Provider delivery is stopping."));
        }
        operation()
    }
    pub async fn run(
        &self,
        operation: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        let receiver = {
            let mut state = self.0.lock().unwrap();
            if state.closed {
                return Ok(());
            }
            if let Some((_, reply)) = &state.active {
                reply.clone()
            } else {
                let id = uuid::Uuid::new_v4();
                let (tx, rx) = watch::channel(None);
                state.active = Some((id, rx.clone()));
                let state = self.0.clone();
                tokio::spawn(async move {
                    struct Clear(Arc<Mutex<State>>, uuid::Uuid);
                    impl Drop for Clear {
                        fn drop(&mut self) {
                            let mut state = self.0.lock().unwrap();
                            if state.active.as_ref().is_some_and(|(id, _)| *id == self.1) {
                                state.active = None;
                            }
                        }
                    }
                    let _clear = Clear(state, id);
                    tx.send_replace(Some(operation.await.map_err(|e| e.to_string())));
                });
                rx
            }
        };
        receive(receiver).await
    }
    pub async fn close(&self) -> Result<()> {
        let receiver = {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            state.active.as_ref().map(|(_, rx)| rx.clone())
        };
        if let Some(receiver) = receiver {
            receive(receiver).await?;
        }
        Ok(())
    }
}
async fn receive(mut receiver: Reply) -> Result<()> {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result.map_err(|e| conflict(&e));
        }
        receiver.changed().await.map_err(|_| {
            conflict("Provider delivery worker was interrupted. Reconcile its saved intents.")
        })?;
    }
}
