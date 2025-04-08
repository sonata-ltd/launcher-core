use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export)]
pub struct Task<'a> {
    pub name: &'a str,
    // descr: Option<&'a str>,
    // is_active: Option<bool>,
    // progress_percentage: Option<bool>,
    // is_done: Option<bool>,
    // is_errored: Option<bool>,
}

impl<'a> Task<'a> {
    pub fn new(
        name: &'a str,
        // descr: Option<&'a str>,
        // is_active: Option<bool>,
        // progress_percentage: Option<bool>,
        // is_done: Option<bool>,
        // is_errored: Option<bool>,
    ) -> Task<'a> {
        Task {
            name,
            // descr,
            // is_active,
            // progress_percentage,
            // is_done,
            // is_errored,
        }
    }
}
