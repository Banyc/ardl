use std::{
    sync::Arc,
    time::{self, Instant},
};

use crate::utils::buf::BufPasta;

pub struct SendingPush {
    body: Arc<BufPasta>,
    last_sent: time::Instant,
    is_retransmitted: bool,
}

impl SendingPush {
    pub fn new(body: Arc<BufPasta>) -> Self {
        SendingPush {
            body,
            last_sent: Instant::now(),
            is_retransmitted: false,
        }
    }

    pub fn body(&self) -> &Arc<BufPasta> {
        &self.body
    }

    pub fn to_retransmit(&mut self) {
        self.last_sent = Instant::now();
        self.is_retransmitted = true;
    }

    pub fn is_timeout(&self, timeout: &time::Duration) -> bool {
        *timeout <= self.last_sent.elapsed()
    }

    pub fn is_retransmitted(&self) -> bool {
        self.is_retransmitted
    }

    pub fn since_last_sent(&self) -> time::Duration {
        Instant::now().duration_since(self.last_sent)
    }
}
