//! The transaction that applies a settings change: prepare, commit, verify, and rollback.
//!
//! The stages are types, so the order is checked by the compiler. Verifying a transaction that
//! was not committed, or committing one that was not prepared, does not compile:
//!
//! ```compile_fail,E0599
//! use screendockd::core::reconfig::{Planned, Reconfig, ReconfigTarget};
//!
//! fn skip_ahead<T: ReconfigTarget>(change: Reconfig<'_, T, Planned>) {
//!     let _ = change.verify();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use screendockd::core::reconfig::{Planned, Reconfig, ReconfigTarget};
//!
//! fn skip_prepare<T: ReconfigTarget>(change: Reconfig<'_, T, Planned>) {
//!     let _ = change.commit();
//! }
//! ```
//!
//! A failure before the commit changes nothing. A failure at the commit or at the verify stage
//! of a change that is more than live puts the previous settings back through the target; if
//! that fails as well the error says so and the caller decides what the pipeline is now.

use std::marker::PhantomData;

use tracing::warn;

use super::types::{
    ChangeClass, Committed, Inner, LiveSettings, Planned, Prepared, Reconfig, ReconfigError,
    ReconfigPlan, ReconfigTarget, Verified,
};

impl<'t, T: ReconfigTarget + ?Sized, S> Reconfig<'t, T, S> {
    fn into_inner(mut self) -> Inner<'t, T> {
        self.inner
            .take()
            .expect("a transaction holds its state until it moves to the next stage")
    }

    fn staged<N>(inner: Inner<'t, T>) -> Reconfig<'t, T, N> {
        Reconfig {
            inner: Some(inner),
            stage: PhantomData,
        }
    }
}

/// Puts the previous settings back after `reason` and says how that went. A live change has
/// nothing to put back: its steps are applied whole or not at all.
fn roll_back<T: ReconfigTarget + ?Sized>(inner: Inner<'_, T>, reason: String) -> ReconfigError {
    if inner.plan.class() == ChangeClass::Live {
        return ReconfigError::Rejected {
            reason,
            rolled_back: false,
        };
    }
    match inner.target.restore(&inner.inverse, &inner.previous) {
        Ok(()) => ReconfigError::Rejected {
            reason,
            rolled_back: true,
        },
        Err(rollback) => ReconfigError::RollbackFailed { reason, rollback },
    }
}

impl<'t, T: ReconfigTarget + ?Sized> Reconfig<'t, T, Planned> {
    /// Starts a change from `previous` to `candidate`. `plan` is the change itself and
    /// `inverse` the change back.
    pub fn plan(
        target: &'t mut T,
        previous: &LiveSettings,
        candidate: &LiveSettings,
        plan: ReconfigPlan,
        inverse: ReconfigPlan,
    ) -> Self {
        Self::staged(Inner {
            target,
            previous: previous.clone(),
            candidate: candidate.clone(),
            plan,
            inverse,
            armed: false,
        })
    }

    /// Builds and checks the new pieces. On an error nothing changed.
    pub fn prepare(self) -> Result<Reconfig<'t, T, Prepared>, ReconfigError> {
        let inner = self.into_inner();
        match inner.target.prepare(&inner.plan, &inner.candidate) {
            Ok(()) => Ok(Reconfig::<T, Prepared>::staged(inner)),
            Err(reason) => Err(ReconfigError::Rejected {
                reason,
                rolled_back: false,
            }),
        }
    }
}

impl<'t, T: ReconfigTarget + ?Sized> Reconfig<'t, T, Prepared> {
    /// Puts the new settings into effect. On an error the previous settings are put back.
    pub fn commit(self) -> Result<Reconfig<'t, T, Committed>, ReconfigError> {
        let mut inner = self.into_inner();
        match inner.target.commit(&inner.plan, &inner.candidate) {
            Ok(()) => {
                inner.armed = inner.plan.class() != ChangeClass::Live;
                Ok(Reconfig::<T, Committed>::staged(inner))
            }
            Err(reason) => Err(roll_back(inner, reason)),
        }
    }
}

impl<'t, T: ReconfigTarget + ?Sized> Reconfig<'t, T, Committed> {
    /// Checks the pipeline with the new settings. On an error the previous settings are put
    /// back.
    pub fn verify(self) -> Result<Verified, ReconfigError> {
        let mut inner = self.into_inner();
        inner.armed = false;
        match inner.target.verify(&inner.plan, &inner.candidate) {
            Ok(()) => Ok(Verified {
                settings: inner.candidate,
            }),
            Err(reason) => Err(roll_back(inner, reason)),
        }
    }
}

impl<T: ReconfigTarget + ?Sized, S> Drop for Reconfig<'_, T, S> {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        if inner.armed {
            if let Err(rollback) = inner.target.restore(&inner.inverse, &inner.previous) {
                warn!(%rollback, "an unverified change was dropped and could not be rolled back");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::reconfig::{plan_change, SettingsPatch};
    use crate::protocol::display::{DisplayGeometry, Orientation};
    use crate::utils::config::load_from_path;

    fn shipped() -> LiveSettings {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/screendock_cfg.toml");
        LiveSettings::try_from(&load_from_path(&path).unwrap()).unwrap()
    }

    /// Records the stages and fails the ones it is told to.
    #[derive(Default)]
    struct Stub {
        calls: Vec<&'static str>,
        fail: Vec<&'static str>,
    }

    impl Stub {
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.fail.contains(&name) {
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }
    }

    impl ReconfigTarget for Stub {
        fn prepare(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("prepare")
        }
        fn commit(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("commit")
        }
        fn verify(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("verify")
        }
        fn restore(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("restore")
        }
    }

    fn change(patch: SettingsPatch) -> (LiveSettings, LiveSettings, ReconfigPlan, ReconfigPlan) {
        let old = shipped();
        let new = patch.apply_to(&old).unwrap();
        let plan = plan_change(&old, &new);
        let inverse = plan_change(&new, &old);
        (old, new, plan, inverse)
    }

    fn resize() -> SettingsPatch {
        SettingsPatch::resolution(1600, 900)
    }

    fn live() -> SettingsPatch {
        SettingsPatch {
            bitrate_kbps: Some(2000),
            ..SettingsPatch::default()
        }
    }

    fn run(stub: &mut Stub, patch: SettingsPatch) -> Result<Verified, ReconfigError> {
        let (old, new, plan, inverse) = change(patch);
        Reconfig::plan(stub, &old, &new, plan, inverse)
            .prepare()
            .and_then(Reconfig::commit)
            .and_then(Reconfig::verify)
    }

    #[test]
    fn a_change_that_works_runs_every_stage_once_in_order() {
        let mut stub = Stub::default();
        let verified = run(&mut stub, resize()).unwrap();
        assert_eq!(stub.calls, ["prepare", "commit", "verify"]);
        assert_eq!(
            verified.settings.geometry,
            DisplayGeometry::new(1600, 900, Orientation::Landscape).unwrap()
        );
    }

    #[test]
    fn a_failed_prepare_stops_before_anything_is_committed_or_restored() {
        let mut stub = Stub {
            fail: vec!["prepare"],
            ..Stub::default()
        };
        let err = run(&mut stub, resize()).unwrap_err();
        assert_eq!(
            err,
            ReconfigError::Rejected {
                reason: "prepare failed".into(),
                rolled_back: false
            }
        );
        assert_eq!(stub.calls, ["prepare"]);
    }

    #[test]
    fn a_failed_commit_restores_the_previous_settings() {
        let mut stub = Stub {
            fail: vec!["commit"],
            ..Stub::default()
        };
        let err = run(&mut stub, resize()).unwrap_err();
        assert_eq!(
            err,
            ReconfigError::Rejected {
                reason: "commit failed".into(),
                rolled_back: true
            }
        );
        assert_eq!(stub.calls, ["prepare", "commit", "restore"]);
    }

    #[test]
    fn a_failed_verify_restores_the_previous_settings() {
        let mut stub = Stub {
            fail: vec!["verify"],
            ..Stub::default()
        };
        let err = run(&mut stub, resize()).unwrap_err();
        assert_eq!(
            err,
            ReconfigError::Rejected {
                reason: "verify failed".into(),
                rolled_back: true
            }
        );
        assert_eq!(stub.calls, ["prepare", "commit", "verify", "restore"]);
    }

    #[test]
    fn a_failed_rollback_is_reported_with_the_original_reason() {
        let mut stub = Stub {
            fail: vec!["commit", "restore"],
            ..Stub::default()
        };
        let err = run(&mut stub, resize()).unwrap_err();
        assert_eq!(
            err,
            ReconfigError::RollbackFailed {
                reason: "commit failed".into(),
                rollback: "restore failed".into()
            }
        );
    }

    #[test]
    fn a_live_change_that_fails_has_nothing_to_roll_back() {
        let mut stub = Stub {
            fail: vec!["commit"],
            ..Stub::default()
        };
        let err = run(&mut stub, live()).unwrap_err();
        assert_eq!(
            err,
            ReconfigError::Rejected {
                reason: "commit failed".into(),
                rolled_back: false
            }
        );
        assert_eq!(stub.calls, ["prepare", "commit"]);
    }

    #[test]
    fn a_committed_change_dropped_unverified_is_rolled_back() {
        let mut stub = Stub::default();
        let (old, new, plan, inverse) = change(resize());
        let committed = Reconfig::plan(&mut stub, &old, &new, plan, inverse)
            .prepare()
            .and_then(Reconfig::commit)
            .unwrap();
        drop(committed);
        assert_eq!(stub.calls, ["prepare", "commit", "restore"]);
    }

    #[test]
    fn a_verified_change_is_not_rolled_back_when_it_goes_out_of_scope() {
        let mut stub = Stub::default();
        run(&mut stub, resize()).unwrap();
        assert!(!stub.calls.contains(&"restore"));
    }
}
