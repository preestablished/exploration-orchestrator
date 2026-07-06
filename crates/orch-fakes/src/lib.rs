#![forbid(unsafe_code)]

//! Deterministic fake service implementations for search-loop testing.
//!
//! This crate owns in-repository fakes that mirror the orchestrator client DTO
//! boundaries without adding transport, async runtime, filesystem, network, or
//! wall-clock dependencies.

pub mod fault;
pub mod grid;
pub mod hypervisor;
pub mod observatory;
pub mod scorer;
pub mod snapshot_store;
pub mod synth;
pub mod transcript;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_exports_fake_modules() {
        #[allow(unused_imports)]
        use crate::{
            fault, grid, hypervisor, observatory, scorer, snapshot_store, synth, transcript,
        };
    }
}
