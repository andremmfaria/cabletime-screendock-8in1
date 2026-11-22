//! Personal/user-specified project conventions, additive to the literature-derived Design
//! Patterns in `docs/implementation-plan.md` (see that doc's Design Patterns section for the
//! explicit note distinguishing the two). Not yet wired into `main.rs`'s orchestration — that's
//! later-milestone integration work — but both submodules compile and are tested standalone.

pub mod config;
pub mod logging;
