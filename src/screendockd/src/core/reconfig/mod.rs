//! Live settings model: the values that can change while the daemon runs, the patch that
//! changes some of them, and the pure classification of a change into what must be done.

pub mod keys;
pub mod plan;
pub mod settings;
pub mod types;

pub use self::plan::{plan_change, plan_change_with};
pub use self::types::{
    ChangeClass, LiveSettings, PlanOptions, ReconfigAction, ReconfigPlan, SettingKey,
    SettingsError, SettingsPatch,
};
