//! Crate-local helpers for deterministic test synchronisation.

use tokio::sync::watch;

pub(crate) fn increment_watch(sender: &watch::Sender<u64>) {
    sender.send_modify(|value| *value += 1);
}

pub(crate) async fn wait_for_watch_minimum(receiver: &mut watch::Receiver<u64>, minimum: u64) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while *receiver.borrow_and_update() < minimum {
            receiver
                .changed()
                .await
                .expect("watch sender must stay alive while waiting for a value");
        }
    })
    .await
    .expect("watch value must reach its minimum within five seconds");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_watch_preserves_values_before_receiver_subscribes() {
        let (sender, receiver) = watch::channel(0);
        drop(receiver);

        increment_watch(&sender);
        increment_watch(&sender);

        let receiver = sender.subscribe();

        assert_eq!(*receiver.borrow(), 2);
    }
}
