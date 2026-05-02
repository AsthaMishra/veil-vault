use crate::{LendingError, constants::MAX_PROTOCOL_FEE_BPS};
use anchor_lang::prelude::*;

#[account]
#[derive(Debug)]
pub struct LendingMarket {
    pub version: u8,

    pub bump: u8,

    pub owner: Pubkey,

    pub emergency_pause: bool,

    pub quote_currency: [u8; 32],

    //bps - basis points
    //1 bps   = 0.01%
    // 10 bps  = 0.1%
    // 50 bps  = 0.5%
    // 100 bps = 1%
    pub protocol_fee_bps: u16,

    pub padding: [u64; 64],
}

impl Default for LendingMarket {
    fn default() -> Self {
        Self {
            version: 0,
            bump: 0,
            owner: Pubkey::default(),
            emergency_pause: false,
            quote_currency: Default::default(),
            protocol_fee_bps: 0,
            padding: [0; 64],
        }
    }
}

pub struct InitializeLendingParams {
    pub bump: u8,
    pub owner: Pubkey,
    pub quote_currency: [u8; 32],
    pub protocol_fee_bps: u16,
}

impl LendingMarket {
    pub fn init(&mut self, params: InitializeLendingParams) -> Result<()> {
        *self = Self::default();
        self.version = 1;
        self.bump = params.bump;
        self.owner = params.owner;
        self.emergency_pause = false;
        self.quote_currency = params.quote_currency;
        self.protocol_fee_bps = params.protocol_fee_bps;

        Ok(())
    }

    pub fn set_version(&mut self, version: u8) -> Result<()> {
        require!(version > self.version, LendingError::InvalidVersion);

        self.version = version;
        Ok(())
    }

    pub fn set_emergency_pause(&mut self, emergency_pause: bool) -> Result<()> {
        self.emergency_pause = emergency_pause;
        Ok(())
    }

    pub fn set_protocol_fees(&mut self, protocol_fees: u16) -> Result<()> {
        require!(protocol_fees <= MAX_PROTOCOL_FEE_BPS, LendingError::InvalidFee); // max 10%

        self.protocol_fee_bps = protocol_fees;
        Ok(())
    }

    pub fn is_paused(&self) -> bool {
        self.emergency_pause
    }
}
