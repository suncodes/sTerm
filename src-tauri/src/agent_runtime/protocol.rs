//! Agent 模式公共协议；原生身份与终端代际分别校验。
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub turn_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub turn_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Capabilities {
    pub send: bool,
    pub stop: bool,
    pub models: bool,
    pub plan: bool,
    pub policy: bool,
    pub approvals: bool,
    pub end: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub protocol_version: u32,
    pub runtime_id: String,
    pub session_id: String,
    pub tab_id: u32,
    pub generation: u32,
    pub agent: String,
    pub title: String,
    pub cwd: String,
    pub model: String,
    pub models: Vec<String>,
    pub policy: String,
    pub state: String,
    pub turn_id: String,
    pub seq: u64,
    pub settings_revision: u64,
    pub shared: bool,
    pub allow_control: bool,
    pub capabilities: Capabilities,
    pub items: Vec<Item>,
    pub approvals: Vec<Approval>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: String,
    pub runtime_id: String,
    pub generation: u32,
    pub session_id: String,
    #[serde(default)]
    pub settings_revision: Option<u64>,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Action {
    SendText { text: String },
    StopTurn { #[serde(rename = "turnId")] turn_id: String },
    SetModel { model: String },
    SetPolicy { sandbox: String, #[serde(rename = "approvalPolicy")] approval_policy: String },
    EnterPlan,
    RespondApproval { #[serde(rename = "requestId")] request_id: String, allow: bool },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub id: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Ready { #[serde(rename = "sessionId")] session_id: String, model: String, models: Vec<String>, capabilities: Capabilities },
    State { state: String, #[serde(default, rename = "turnId")] turn_id: String },
    Items { items: Vec<Item> },
    Upsert { item: Item },
    Delta { id: String, delta: String },
    Settings { model: Option<String>, policy: Option<String> },
    Approval { approval: Approval },
    Resolved { id: String },
    Result { id: String, ok: bool, message: Option<String> },
    Error { message: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub session_id: String,
    pub events: Vec<Value>,
}


#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub seq: u64,
    pub events: Vec<Event>,
}
