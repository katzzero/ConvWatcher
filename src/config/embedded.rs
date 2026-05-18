use serde::{Deserialize, Serialize};

use super::watch::WatchType;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddedConfig {
    pub secret: String,

    #[serde(default)]
    pub output_folder: String,

    #[serde(flatten)]
    pub watch_type: WatchType,
}
