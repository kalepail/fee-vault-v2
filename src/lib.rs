#![no_std]

#[cfg(any(test, feature = "testutils"))]
extern crate std;
#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub mod constants;
pub mod contract;
pub mod errors;
pub mod events;
pub mod pool;
pub mod rewards;
pub mod storage;
pub mod validator;
pub mod vault;

pub use contract::*;

#[cfg(test)]
mod tests;
