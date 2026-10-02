use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod app_api;
pub mod balance;
pub mod codex_auth;
pub mod codex_catalog;
pub mod codex_config;
pub mod domain;
pub mod incident;
pub mod lifecycle;
pub mod pricing;
pub mod pricing_capture;
pub mod pricing_local;
pub mod proxy;
pub mod qa_acceptance;
pub mod recovery;
pub mod runtime_log;
pub mod state;
pub mod storage;
pub mod upstream_models;

pub const APP_NAME: &str = "AI Router";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[ts(rename_all = "camelCase")]
pub struct BuildInfoDto {
    pub app_name: String,
    pub api_version: u32,
}

impl Default for BuildInfoDto {
    fn default() -> Self {
        Self {
            app_name: APP_NAME.to_owned(),
            api_version: 1,
        }
    }
}
