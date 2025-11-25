use std::thread;
use tokio::sync::mpsc;
use zmq::Context;

pub struct ZmqListener {
    url: String,
    sender: mpsc::Sender<()>,
}

impl ZmqListener {
    pub fn new(url: String, sender: mpsc::Sender<()>) -> Self {
        Self { url, sender }
    }

    pub fn start(self) {
        let url = self.url.clone();
        let sender = self.sender.clone();

        thread::spawn(move || {
            let context = Context::new();
            let subscriber = context
                .socket(zmq::SUB)
                .expect("Failed to create ZMQ socket");

            tracing::info!("Connecting to ZMQ at {}", url);
            subscriber.connect(&url).expect("Failed to connect to ZMQ");

            // Subscribe to rawblock notifications (hashblock works as a fallback)
            subscriber
                .set_subscribe(b"rawblock")
                .expect("Failed to subscribe");
            subscriber.set_subscribe(b"hashblock").ok();

            loop {
                // Consume the topic frame and the raw payload frame
                if subscriber.recv_msg(0).is_ok() {
                    if subscriber.recv_msg(0).is_ok() {
                        // Signal the async loop so it rechecks RPC height
                        if let Err(_) = sender.blocking_send(()) {
                            tracing::info!("ZMQ receiver dropped, stopping listener");
                            break;
                        }
                    }
                }
            }
        });
    }
}
