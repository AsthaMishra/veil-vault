use crate::constants::MAX_AGE_SECONDS;
use anchor_lang::prelude::*;


#[derive(Debug)]
#[zero_copy]
#[repr(C)]
pub struct LastUpdate {
    pub slot: u64,
    pub timestamp: i64,
}

impl Default for LastUpdate {
    fn default() -> Self {
        Self {
            slot: 0,
            timestamp: 0,
        }
    }
}

impl LastUpdate {
    pub fn init(&mut self) {
        *self = Self::default();
    }

    pub fn update(&mut self, slot: u64, timestamp: i64) {
        self.slot = slot;
        self.timestamp = timestamp;
    }

    pub fn is_price_stale(&self, curr_time: i64) -> bool {
        curr_time.saturating_sub(self.timestamp) > MAX_AGE_SECONDS
    }

    pub fn is_slot_stale(&self, curr_slot: u64) -> bool {
        curr_slot != self.slot
    }
}
