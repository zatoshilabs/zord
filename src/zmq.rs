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

            // Subscribe to rawblock (or hashblock if available, but rawblock is confirmed in config)
            // We just use it as a trigger, so we don't care about the content for now.
            subscriber
                .set_subscribe(b"rawblock")
                .expect("Failed to subscribe");
            subscriber.set_subscribe(b"hashblock").ok(); // Try this too just in case

            loop {
                // ZMQ multipart: [topic, body]
                if subscriber.recv_msg(0).is_ok() {
                    // We don't strictly need the body if we just use this as a trigger
                    if subscriber.recv_msg(0).is_ok() {
                        // Notify indexer
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
