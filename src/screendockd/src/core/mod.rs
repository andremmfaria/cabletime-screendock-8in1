//! Core logic that depends only on the ports and the protocol: the dock session and its timing
//! policy, and the live settings model with its change classification.

pub mod policy;
pub mod reconfig;
pub mod session;
pub mod types;
