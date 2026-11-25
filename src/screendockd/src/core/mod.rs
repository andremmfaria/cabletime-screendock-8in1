//! Core logic that depends only on the ports and the protocol: the dock session and its timing
//! policy, the live settings model with its change classification, and the display policy.

pub mod display_policy;
pub mod keyframe;
pub mod policy;
pub mod reconfig;
pub mod session;
pub mod types;
