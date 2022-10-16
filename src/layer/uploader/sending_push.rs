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
    #[must_use]
    pub fn new(body: Arc<BufPasta>, now: Instant) -> Self {
        SendingPush {
            body,
            last_sent: now,
            is_retransmitted: false,
        }
    }

    #[must_use]
    pub fn body(&self) -> &Arc<BufPasta> {
        &self.body
    }

    pub fn to_retransmit(&mut self, now: Instant) {
        self.last_sent = now;
        self.is_retransmitted = true;
    }

    // #[must_use]
    // pub fn is_timeout(&self, timeout: &time::Duration) -> bool {
    //     *timeout <= self.last_sent.elapsed()
    // }

    #[must_use]
    pub fn last_sent(&self) -> Instant {
        self.last_sent
    }

    #[must_use]
    pub fn is_retransmitted(&self) -> bool {
        self.is_retransmitted
    }

    #[must_use]
    pub fn since_last_sent(&self, now: &Instant) -> time::Duration {
        now.duration_since(self.last_sent)
    }
}
