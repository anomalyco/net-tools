use n0_error::{e, stack_error};
use tokio::sync::{mpsc, watch};

use super::actor::NetworkMessage;
use crate::{
    interfaces::{State, ios::state_for},
    ios::{Path, PathMonitor},
};

#[stack_error(derive, add_meta)]
#[non_exhaustive]
pub enum Error {
    #[error("could not start iOS network path monitor (requires iOS 12 or later)")]
    Unavailable {},
}

#[derive(Debug)]
pub(super) struct RouteMonitor {
    _monitor: PathMonitor,
    path: watch::Receiver<Option<Path>>,
}

impl RouteMonitor {
    pub(super) fn new(sender: mpsc::Sender<NetworkMessage>) -> Result<Self, Error> {
        let (monitor, path) = PathMonitor::subscribe(move || {
            // Never block the dispatch queue. Full means a rescan is already
            // pending; it will use the latest path, even if we coalesce here.
            let _ = sender.try_send(NetworkMessage::Change);
        })
        .ok_or_else(|| e!(Error::Unavailable))?;
        Ok(Self {
            _monitor: monitor,
            path,
        })
    }

    pub(super) async fn state(&mut self) -> State {
        state_for(Path::latest(&mut self.path).await)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn native_notification_reaches_actor_and_releases_sender() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut monitor = RouteMonitor::new(tx).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            assert!(matches!(rx.recv().await, Some(NetworkMessage::Change)));
            let state = monitor.state().await;
            assert!(
                state
                    .local_addresses
                    .loopback
                    .iter()
                    .any(|ip| ip.is_loopback())
            );
            if let Some(route) = state.default_route_interface {
                assert!(state.interfaces.contains_key(&route));
            }
            drop(monitor);
            while rx.recv().await.is_some() {}
        })
        .await
        .expect("native update and cancellation callbacks");
    }
}
