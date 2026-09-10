use tokio::sync::broadcast;
use crate::routes::jobs::JobResponse;

#[derive(Clone)]
pub struct Broadcaster {
    tx: broadcast::Sender<JobResponse>,
}

impl Broadcaster {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn send(&self, job: JobResponse) {
        let _ = self.tx.send(job);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobResponse> {
        self.tx.subscribe()
    }
}
