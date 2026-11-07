//! Fixed one-shot shared preference command. Provider qualification stays sealed.
pub use crate::storage::sync::selected::{
    run_preferences as run, PreferenceOperation as Operation, PreferenceReport as Report,
};
