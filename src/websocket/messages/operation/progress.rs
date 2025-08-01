use serde::{Deserialize, Serialize};
use ts_rs::TS;


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub enum ProgressUnit {
    Bytes,
    Items,
    Percent,
}
