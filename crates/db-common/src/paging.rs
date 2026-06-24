use crate::{validation, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_SIZE: u32 = 500;
pub const MAX_PAGE_SIZE: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSize(u32);

impl PageSize {
    pub fn new(value: u32) -> Result<Self> {
        validation::ensure_positive_limit("page_size", value)?;

        if value > MAX_PAGE_SIZE {
            return Ok(Self(MAX_PAGE_SIZE));
        }

        Ok(Self(value))
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self(DEFAULT_PAGE_SIZE)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub offset: u64,
    pub size: PageSize,
}

impl PageRequest {
    pub const fn first() -> Self {
        Self {
            offset: 0,
            size: PageSize(DEFAULT_PAGE_SIZE),
        }
    }

    pub fn new(offset: u64, size: u32) -> Result<Self> {
        Ok(Self {
            offset,
            size: PageSize::new(size)?,
        })
    }

    pub fn next(self) -> Self {
        Self {
            offset: self.offset + u64::from(self.size.get()),
            size: self.size,
        }
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first()
    }
}
