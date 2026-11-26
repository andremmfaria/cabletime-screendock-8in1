//! The supervisor: the one owner of the daemon's live settings.
//!
//! Every front end (the control socket, a config reload, SIGHUP) sends a
//! [`Command`](crate::ports::control::Command) to the supervisor. It validates the change,
//! classifies it with [`plan_change_with`] and either hands it to the pipeline or rejects it,
//! so no other thread mutates settings and every route shares one path. Policy lives here;
//! the pipeline handle only executes steps.
//!
//! A change runs as a [`Reconfig`] transaction: prepare, commit, verify. A change that fails
//! at the commit or verify stage is rolled back to the settings that last worked and answered
//! `rejected` with `rolled_back`. If the rollback fails as well the daemon is degraded: the
//! settings stay at the last good ones, `status` says so, and the next change or reload first
//! tries to bring the pipeline back to them.

use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;

use tracing::{info, info_span, warn};

pub use super::types::{NullPipeline, PipelineHandle, ReloadFn, Supervisor};
use crate::core::reconfig::keys::{answer_read, changed_keys};
use crate::core::reconfig::{
    plan_change_with, ChangeClass, LiveSettings, PlanOptions, Reconfig, ReconfigAction,
    ReconfigError, ReconfigPlan, ReconfigTarget, SettingKey, SettingsPatch,
};
use crate::ports::control::Command;
use crate::protocol::control::{ControlRequest, ControlResponse};

impl<T: PipelineHandle + ?Sized> ReconfigTarget for T {
    fn prepare(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        PipelineHandle::prepare(self, plan, settings)
    }

    fn commit(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        self.apply(plan, settings)
    }

    fn verify(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        PipelineHandle::verify(self, plan, settings)
    }

    fn restore(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
        PipelineHandle::restore(self, plan, settings)
    }
}

impl PipelineHandle for NullPipeline {
    fn supported(&self) -> Vec<ReconfigAction> {
        vec![ReconfigAction::SetFollow]
    }

    fn options(&self) -> PlanOptions {
        PlanOptions { live_rate: false }
    }

    fn apply(&mut self, _plan: &ReconfigPlan, _settings: &LiveSettings) -> Result<(), String> {
        Ok(())
    }
}

fn class_name(class: ChangeClass) -> &'static str {
    match class {
        ChangeClass::Live => "live",
        ChangeClass::Rebuild => "rebuild",
        ChangeClass::DockCycle => "dock cycle",
    }
}

fn action_name(action: ReconfigAction) -> &'static str {
    match action {
        ReconfigAction::SetRate => "set rate",
        ReconfigAction::SetKeyframeInterval => "set keyframe interval",
        ReconfigAction::SetFollow => "set follow",
        ReconfigAction::DestroyMonitor => "destroy monitor",
        ReconfigAction::CreateMonitor => "create monitor",
        ReconfigAction::RecreateMonitor => "recreate monitor",
        ReconfigAction::ResizeMonitor => "resize monitor",
        ReconfigAction::RebuildEncoder => "rebuild encoder",
        ReconfigAction::RetargetCapture => "retarget capture",
        ReconfigAction::RunDockCycle => "run dock cycle",
    }
}

impl Supervisor {
    /// A supervisor for `live`, driving `pipeline`. `reload` produces the settings the config
    /// file holds now.
    pub fn new(live: LiveSettings, reload: ReloadFn, pipeline: Box<dyn PipelineHandle>) -> Self {
        Self {
            live,
            reload,
            pipeline,
            extra: BTreeMap::new(),
            changes_applied: 0,
            degraded: None,
        }
    }

    /// Adds a fixed field to the `status` answer.
    pub fn with_status_field(mut self, key: &str, value: impl Into<String>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    /// The settings the daemon is running with: the last ones that worked.
    pub fn settings(&self) -> &LiveSettings {
        &self.live
    }

    /// Why the daemon is degraded, when it is.
    pub fn degraded(&self) -> Option<&str> {
        self.degraded.as_deref()
    }

    /// Serves commands until every sender is gone.
    pub fn run(mut self, inbox: &Receiver<Command>) {
        let span = info_span!("supervisor");
        let _guard = span.enter();
        while let Ok(command) = inbox.recv() {
            self.handle(command);
        }
    }

    /// Handles one command. A request is answered through its reply channel.
    pub fn handle(&mut self, command: Command) {
        match command {
            Command::Request { request, reply } => {
                let response = self.respond(&request);
                let _ = reply.send(response);
            }
            Command::Reload => match self.reload_from_file() {
                ControlResponse::Applied { class, changed } => {
                    info!(class, changed = ?changed, "config reloaded");
                }
                ControlResponse::Rejected { reason, .. } | ControlResponse::Invalid { reason } => {
                    warn!(%reason, "config reload failed");
                }
                other => warn!(?other, "unexpected reload answer"),
            },
        }
    }

    fn respond(&mut self, request: &ControlRequest) -> ControlResponse {
        match request {
            ControlRequest::Set { key, value } => {
                let patch = SettingKey::parse(key).and_then(|key| key.patch(value));
                match patch {
                    Ok(patch) => self.apply(&patch),
                    Err(e) => ControlResponse::invalid(e.to_string()),
                }
            }
            ControlRequest::Reload {} => self.reload_from_file(),
            ControlRequest::Status {} | ControlRequest::Get { .. } => {
                let read_only = self.pipeline.display_fields();
                if let ControlRequest::Get { key: Some(key) } = request {
                    if let Some(value) = read_only.get(key) {
                        return ControlResponse::Values {
                            values: BTreeMap::from([(key.clone(), value.clone())]),
                        };
                    }
                }
                let mut extra = self.extra.clone();
                if matches!(request, ControlRequest::Status {}) {
                    extra.extend(read_only.clone());
                }
                extra.insert("changes_applied".into(), self.changes_applied.to_string());
                extra.insert(
                    "state".into(),
                    if self.degraded.is_some() {
                        "degraded"
                    } else {
                        "running"
                    }
                    .into(),
                );
                if let Some(reason) = &self.degraded {
                    extra.insert("degraded_reason".into(), reason.clone());
                }
                let mut answer =
                    answer_read(&self.live, request, extra).expect("read requests are answered");
                if let (ControlResponse::Values { values }, ControlRequest::Get { key: None }) =
                    (&mut answer, request)
                {
                    values.extend(read_only);
                }
                answer
            }
        }
    }

    fn reload_from_file(&mut self) -> ControlResponse {
        match (self.reload)() {
            Ok(new) => {
                let patch = SettingsPatch::between(&self.live, &new);
                self.apply(&patch)
            }
            Err(reason) => ControlResponse::rejected(format!("config reload failed: {reason}")),
        }
    }

    /// Validates, classifies and applies a patch as a transaction. The settings change only
    /// when every stage worked.
    fn apply(&mut self, patch: &SettingsPatch) -> ControlResponse {
        let span = info_span!("reconfig");
        let _guard = span.enter();
        let candidate = match patch.apply_to(&self.live) {
            Ok(candidate) => candidate,
            Err(e) => return ControlResponse::invalid(e.to_string()),
        };
        if let Some(reason) = self.degraded.clone() {
            // An explicit command is the retry: the pipeline goes back to the last good
            // settings before anything new is attempted.
            if let Err(error) = self.pipeline.resync(&self.live) {
                warn!(%reason, %error, "the retry of a degraded daemon failed");
                return ControlResponse::rejected(format!(
                    "the daemon is degraded ({reason}) and the retry failed: {error}"
                ));
            }
            info!("the pipeline is back at the last good settings");
            self.degraded = None;
        }
        let options = self.pipeline.options();
        let plan = plan_change_with(&self.live, &candidate, options);
        let changed: Vec<String> = changed_keys(&self.live, &candidate)
            .into_iter()
            .map(String::from)
            .collect();
        let class = class_name(plan.class()).to_string();
        if changed.is_empty() {
            return ControlResponse::Applied { class, changed };
        }

        let supported = self.pipeline.supported();
        let missing: Vec<&str> = plan
            .actions()
            .iter()
            .filter(|action| !supported.contains(action))
            .map(|action| action_name(*action))
            .collect();
        if !missing.is_empty() {
            return ControlResponse::rejected(format!(
                "a {class} change needs steps the running pipeline cannot do: {}",
                missing.join(", ")
            ));
        }

        let inverse = plan_change_with(&candidate, &self.live, options);
        let outcome = Reconfig::plan(&mut *self.pipeline, &self.live, &candidate, plan, inverse)
            .prepare()
            .and_then(Reconfig::commit)
            .and_then(Reconfig::verify);
        match outcome {
            Ok(verified) => {
                info!(class = %class, changed = ?changed, "settings changed");
                self.live = verified.settings;
                self.changes_applied += 1;
                ControlResponse::Applied { class, changed }
            }
            Err(ReconfigError::Rejected {
                reason,
                rolled_back,
            }) => {
                warn!(%reason, rolled_back, "change rejected");
                ControlResponse::Rejected {
                    reason,
                    rolled_back,
                }
            }
            Err(ReconfigError::RollbackFailed { reason, rollback }) => {
                warn!(%reason, %rollback, "rollback failed, the daemon is degraded");
                let detail = format!("{reason}; the rollback failed too: {rollback}");
                self.degraded = Some(detail.clone());
                ControlResponse::rejected(format!(
                    "{detail}; the daemon is degraded and keeps the last good settings"
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::config::load_from_path;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    fn shipped() -> LiveSettings {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/screendock_cfg.toml");
        LiveSettings::try_from(&load_from_path(&path).unwrap()).unwrap()
    }

    /// A pipeline that supports a fixed set of steps and records what it was asked to do.
    struct Recording {
        supported: Vec<ReconfigAction>,
        live_rate: bool,
        applied: Arc<Mutex<Vec<Vec<ReconfigAction>>>>,
        fail_with: Option<String>,
    }

    impl PipelineHandle for Recording {
        fn supported(&self) -> Vec<ReconfigAction> {
            self.supported.clone()
        }
        fn options(&self) -> PlanOptions {
            PlanOptions {
                live_rate: self.live_rate,
            }
        }
        fn apply(&mut self, plan: &ReconfigPlan, _settings: &LiveSettings) -> Result<(), String> {
            self.applied.lock().unwrap().push(plan.actions().to_vec());
            self.fail_with.clone().map_or(Ok(()), Err)
        }
    }

    type Applied = Arc<Mutex<Vec<Vec<ReconfigAction>>>>;

    fn supervisor_with(
        supported: &[ReconfigAction],
        reload: ReloadFn,
        fail_with: Option<&str>,
    ) -> (Supervisor, Applied) {
        let applied = Applied::default();
        let pipeline = Recording {
            supported: supported.to_vec(),
            live_rate: true,
            applied: Arc::clone(&applied),
            fail_with: fail_with.map(String::from),
        };
        (
            Supervisor::new(shipped(), reload, Box::new(pipeline)),
            applied,
        )
    }

    fn live_pipeline() -> (Supervisor, Applied) {
        supervisor_with(
            &[
                ReconfigAction::SetRate,
                ReconfigAction::SetKeyframeInterval,
                ReconfigAction::SetFollow,
            ],
            Box::new(|| Ok(shipped())),
            None,
        )
    }

    fn ask(supervisor: &mut Supervisor, request: ControlRequest) -> ControlResponse {
        let (reply, answer) = mpsc::channel();
        supervisor.handle(Command::Request { request, reply });
        answer.recv().unwrap()
    }

    fn set(key: &str, value: &str) -> ControlRequest {
        ControlRequest::Set {
            key: key.into(),
            value: value.into(),
        }
    }

    #[test]
    fn live_change_is_applied_reported_and_visible_in_get() {
        let (mut supervisor, applied) = live_pipeline();
        match ask(&mut supervisor, set("bitrate", "2000")) {
            ControlResponse::Applied { class, changed } => {
                assert_eq!(class, "live");
                assert_eq!(changed, ["bitrate"]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings().bitrate_kbps, 2000);
        assert_eq!(
            applied.lock().unwrap().as_slice(),
            [vec![ReconfigAction::SetRate]]
        );
        match ask(
            &mut supervisor,
            ControlRequest::Get {
                key: Some("bitrate".into()),
            },
        ) {
            ControlResponse::Values { values } => assert_eq!(values["bitrate"], "2000"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn setting_the_current_value_changes_nothing_and_does_not_touch_the_pipeline() {
        let (mut supervisor, applied) = live_pipeline();
        match ask(&mut supervisor, set("bitrate", "4000")) {
            ControlResponse::Applied { changed, .. } => assert!(changed.is_empty()),
            other => panic!("{other:?}"),
        }
        assert!(applied.lock().unwrap().is_empty());
    }

    #[test]
    fn change_the_pipeline_cannot_do_is_rejected_and_leaves_settings_alone() {
        let (mut supervisor, applied) = live_pipeline();
        let before = supervisor.settings().clone();
        match ask(&mut supervisor, set("resolution", "1600x900")) {
            ControlResponse::Rejected {
                reason,
                rolled_back,
            } => {
                assert!(!rolled_back);
                assert!(reason.contains("dock cycle"), "{reason}");
                assert!(reason.contains("rebuild encoder"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings(), &before);
        assert!(applied.lock().unwrap().is_empty());
    }

    #[test]
    fn bitrate_is_rebuild_class_when_the_pipeline_has_no_live_rate() {
        let mut supervisor = Supervisor::new(
            shipped(),
            Box::new(|| Ok(shipped())),
            Box::new(NullPipeline),
        );
        match ask(&mut supervisor, set("bitrate", "2000")) {
            ControlResponse::Rejected { reason, .. } => {
                assert!(reason.contains("a rebuild change"), "{reason}");
                assert!(reason.contains("rebuild encoder"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings().bitrate_kbps, 4000);
    }

    #[test]
    fn pipeline_failure_rejects_the_change_and_keeps_the_old_settings() {
        let (mut supervisor, _) = supervisor_with(
            &[ReconfigAction::SetRate],
            Box::new(|| Ok(shipped())),
            Some("encoder refused"),
        );
        match ask(&mut supervisor, set("bitrate", "2000")) {
            ControlResponse::Rejected { reason, .. } => assert_eq!(reason, "encoder refused"),
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings().bitrate_kbps, 4000);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_invalid() {
        let (mut supervisor, applied) = live_pipeline();
        for request in [
            set("volume", "3"),
            set("bitrate", "lots"),
            set("bitrate", "0"),
            set("keyframe-interval", "99999"),
            set("resolution", "1281x720"),
            set("name", "bad\nname"),
        ] {
            assert!(
                matches!(
                    ask(&mut supervisor, request.clone()),
                    ControlResponse::Invalid { .. }
                ),
                "{request:?}"
            );
        }
        assert!(applied.lock().unwrap().is_empty());
        assert_eq!(supervisor.settings(), &shipped());
    }

    #[test]
    fn reload_applies_only_the_differences_to_the_live_settings() {
        let file_settings = Arc::new(Mutex::new(shipped()));
        let source = Arc::clone(&file_settings);
        let (mut supervisor, applied) = supervisor_with(
            &[
                ReconfigAction::SetRate,
                ReconfigAction::SetKeyframeInterval,
                ReconfigAction::SetFollow,
            ],
            Box::new(move || Ok(source.lock().unwrap().clone())),
            None,
        );
        // A runtime change the file does not know about.
        ask(&mut supervisor, set("keyframe-interval", "10"));
        applied.lock().unwrap().clear();

        {
            let mut file = file_settings.lock().unwrap();
            file.bitrate_kbps = 1500;
            file.follow_system_changes = false;
        }
        match ask(&mut supervisor, ControlRequest::Reload {}) {
            ControlResponse::Applied { class, changed } => {
                assert_eq!(class, "live");
                // The file's keyframe interval differs from the runtime one, so it is part of
                // the diff as well: a reload brings the live settings to the file's values.
                assert_eq!(
                    changed,
                    ["bitrate", "keyframe-interval", "follow-system-changes"]
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings(), &*file_settings.lock().unwrap());
        assert_eq!(applied.lock().unwrap().len(), 1);
    }

    #[test]
    fn reload_with_an_unsupported_difference_applies_nothing() {
        let file_settings = Arc::new(Mutex::new(shipped()));
        let source = Arc::clone(&file_settings);
        let (mut supervisor, applied) = supervisor_with(
            &[ReconfigAction::SetRate, ReconfigAction::SetFollow],
            Box::new(move || Ok(source.lock().unwrap().clone())),
            None,
        );
        {
            let mut file = file_settings.lock().unwrap();
            file.bitrate_kbps = 1500;
            file.fps = 10;
        }
        assert!(matches!(
            ask(&mut supervisor, ControlRequest::Reload {}),
            ControlResponse::Rejected { .. }
        ));
        assert_eq!(supervisor.settings(), &shipped());
        assert!(applied.lock().unwrap().is_empty());
    }

    #[test]
    fn reload_failure_is_reported_and_keeps_the_settings() {
        let (mut supervisor, _) =
            supervisor_with(&[], Box::new(|| Err("syntax error at line 3".into())), None);
        match ask(&mut supervisor, ControlRequest::Reload {}) {
            ControlResponse::Rejected { reason, .. } => {
                assert!(reason.contains("syntax error at line 3"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(supervisor.settings(), &shipped());
    }

    #[test]
    fn signal_reload_takes_the_same_path_without_a_reply() {
        let file_settings = Arc::new(Mutex::new(shipped()));
        let source = Arc::clone(&file_settings);
        let (mut supervisor, _) = supervisor_with(
            &[ReconfigAction::SetFollow],
            Box::new(move || Ok(source.lock().unwrap().clone())),
            None,
        );
        file_settings.lock().unwrap().follow_system_changes = false;
        supervisor.handle(Command::Reload);
        assert!(!supervisor.settings().follow_system_changes);
    }

    struct WithDisplay;

    impl PipelineHandle for WithDisplay {
        fn supported(&self) -> Vec<ReconfigAction> {
            NullPipeline.supported()
        }
        fn options(&self) -> PlanOptions {
            NullPipeline.options()
        }
        fn apply(&mut self, plan: &ReconfigPlan, settings: &LiveSettings) -> Result<(), String> {
            NullPipeline.apply(plan, settings)
        }
        fn display_fields(&self) -> BTreeMap<String, String> {
            BTreeMap::from([
                ("edid-name".to_string(), "Panel".to_string()),
                ("edid-modes".to_string(), "1280x720@60".to_string()),
            ])
        }
    }

    #[test]
    fn display_facts_are_read_only_keys_of_status_and_get() {
        let mut supervisor = Supervisor::new(
            shipped(),
            Box::new(|| Err("unused".to_string())),
            Box::new(WithDisplay),
        );
        match ask(&mut supervisor, ControlRequest::Status {}) {
            ControlResponse::Status { fields } => assert_eq!(fields["edid-name"], "Panel"),
            other => panic!("{other:?}"),
        }
        let one = ControlRequest::Get {
            key: Some("edid-modes".into()),
        };
        match ask(&mut supervisor, one) {
            ControlResponse::Values { values } => {
                assert_eq!(values.len(), 1);
                assert_eq!(values["edid-modes"], "1280x720@60");
            }
            other => panic!("{other:?}"),
        }
        match ask(&mut supervisor, ControlRequest::Get { key: None }) {
            ControlResponse::Values { values } => {
                assert_eq!(values["edid-name"], "Panel");
                assert_eq!(values["mode"], "mirror");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            ask(&mut supervisor, set("edid-name", "x")),
            ControlResponse::Invalid { .. }
        ));
    }

    #[test]
    fn status_carries_the_extra_fields_and_the_change_count() {
        let (supervisor, _) = live_pipeline();
        let mut supervisor = supervisor.with_status_field("version", "9.9.9");
        ask(&mut supervisor, set("bitrate", "3000"));
        match ask(&mut supervisor, ControlRequest::Status {}) {
            ControlResponse::Status { fields } => {
                assert_eq!(fields["version"], "9.9.9");
                assert_eq!(fields["changes_applied"], "1");
                assert_eq!(fields["bitrate"], "3000");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn run_serves_commands_until_the_senders_are_gone() {
        let (supervisor, _) = live_pipeline();
        let (commands, inbox) = mpsc::channel();
        let thread = std::thread::spawn(move || supervisor.run(&inbox));
        let (reply, answer) = mpsc::channel();
        commands
            .send(Command::Request {
                request: set("keyframe-interval", "20"),
                reply,
            })
            .unwrap();
        assert!(matches!(
            answer.recv().unwrap(),
            ControlResponse::Applied { .. }
        ));
        drop(commands);
        thread.join().unwrap();
    }

    /// A pipeline that records every stage and fails the ones it is told to.
    struct Staged {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Staged {
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.lock().unwrap().push(name);
            if self.fail.lock().unwrap().contains(&name) {
                Err(format!("{name} refused"))
            } else {
                Ok(())
            }
        }
    }

    impl PipelineHandle for Staged {
        fn supported(&self) -> Vec<ReconfigAction> {
            vec![
                ReconfigAction::SetRate,
                ReconfigAction::RebuildEncoder,
                ReconfigAction::RunDockCycle,
                ReconfigAction::RetargetCapture,
            ]
        }
        fn options(&self) -> PlanOptions {
            PlanOptions { live_rate: true }
        }
        fn apply(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("commit")
        }
        fn prepare(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("prepare")
        }
        fn verify(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("verify")
        }
        fn restore(&mut self, _: &ReconfigPlan, _: &LiveSettings) -> Result<(), String> {
            self.step("restore")
        }
        fn resync(&mut self, _: &LiveSettings) -> Result<(), String> {
            self.step("resync")
        }
    }

    type Calls = Arc<Mutex<Vec<&'static str>>>;

    fn staged(fail: &[&'static str]) -> (Supervisor, Calls) {
        let (supervisor, calls, _) = staged_with_switch(fail);
        (supervisor, calls)
    }

    /// Like `staged`, and the list of stages that fail can be changed afterwards.
    fn staged_with_switch(fail: &[&'static str]) -> (Supervisor, Calls, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(Mutex::new(fail.to_vec()));
        let pipeline = Staged {
            calls: Arc::clone(&calls),
            fail: Arc::clone(&failing),
        };
        (
            Supervisor::new(shipped(), Box::new(|| Ok(shipped())), Box::new(pipeline)),
            calls,
            failing,
        )
    }

    #[test]
    fn a_change_runs_prepare_commit_and_verify_in_order() {
        let (mut supervisor, calls) = staged(&[]);
        assert!(matches!(
            ask(&mut supervisor, set("resolution", "1600x900")),
            ControlResponse::Applied { .. }
        ));
        assert_eq!(*calls.lock().unwrap(), ["prepare", "commit", "verify"]);
        assert_eq!(supervisor.settings().geometry.width().0, 1600);
    }

    #[test]
    fn a_failed_prepare_changes_nothing_and_is_not_a_rollback() {
        let (mut supervisor, calls) = staged(&["prepare"]);
        match ask(&mut supervisor, set("resolution", "1600x900")) {
            ControlResponse::Rejected {
                reason,
                rolled_back,
            } => {
                assert_eq!(reason, "prepare refused");
                assert!(!rolled_back);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(*calls.lock().unwrap(), ["prepare"]);
        assert_eq!(supervisor.settings(), &shipped());
    }

    #[test]
    fn a_failed_commit_or_verify_rolls_back_and_keeps_the_old_settings() {
        for stage in ["commit", "verify"] {
            let (mut supervisor, calls) = staged(&[stage]);
            match ask(&mut supervisor, set("resolution", "1600x900")) {
                ControlResponse::Rejected {
                    reason,
                    rolled_back,
                } => {
                    assert_eq!(reason, format!("{stage} refused"));
                    assert!(rolled_back, "{stage}");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(calls.lock().unwrap().last(), Some(&"restore"), "{stage}");
            assert_eq!(supervisor.settings(), &shipped());
            assert!(supervisor.degraded().is_none());
        }
    }

    #[test]
    fn a_failed_rollback_degrades_the_daemon_and_the_next_command_resyncs_first() {
        let (mut supervisor, calls, failing) = staged_with_switch(&["commit", "restore"]);
        match ask(&mut supervisor, set("resolution", "1600x900")) {
            ControlResponse::Rejected {
                reason,
                rolled_back,
            } => {
                assert!(!rolled_back);
                assert!(reason.contains("degraded"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(supervisor.degraded().is_some());
        assert_eq!(supervisor.settings(), &shipped());
        match ask(&mut supervisor, ControlRequest::Status {}) {
            ControlResponse::Status { fields } => {
                assert_eq!(fields["state"], "degraded");
                assert!(fields["degraded_reason"].contains("restore refused"));
            }
            other => panic!("{other:?}"),
        }

        calls.lock().unwrap().clear();
        failing.lock().unwrap().clear();
        assert!(matches!(
            ask(&mut supervisor, set("bitrate", "2000")),
            ControlResponse::Applied { .. }
        ));
        assert_eq!(
            *calls.lock().unwrap(),
            ["resync", "prepare", "commit", "verify"]
        );
        assert!(supervisor.degraded().is_none());
    }

    #[test]
    fn a_live_change_the_pipeline_refuses_is_not_a_rollback() {
        let (mut supervisor, calls) = staged(&["commit"]);
        match ask(&mut supervisor, set("bitrate", "2000")) {
            ControlResponse::Rejected {
                reason,
                rolled_back,
            } => {
                assert_eq!(reason, "commit refused");
                assert!(!rolled_back);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(*calls.lock().unwrap(), ["prepare", "commit"]);
        assert!(supervisor.degraded().is_none());
    }

    #[test]
    fn keys_of_every_class_are_classified_through_one_path() {
        let (mut supervisor, _) = live_pipeline();
        for key in SettingKey::ALL {
            let current = key.get(supervisor.settings());
            // Setting a key to its current value never fails validation.
            assert!(
                matches!(
                    ask(&mut supervisor, set(key.name(), &current)),
                    ControlResponse::Applied { .. }
                ),
                "{}",
                key.name()
            );
        }
    }
}
