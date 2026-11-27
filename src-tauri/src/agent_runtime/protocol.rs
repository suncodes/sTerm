//! Agent 模式公共协议；原生身份、交互请求与终端代际分别校验。
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(default)] pub title: String,
    #[serde(default)] pub status: String,
    #[serde(default)] pub turn_id: String,
    #[serde(default)] pub agent_id: String,
    #[serde(default)] pub data: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub title: String,
    pub detail: String,
    #[serde(default)] pub turn_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)] pub detail: String,
    #[serde(default)] pub turn_id: String,
    #[serde(default)] pub agent_id: String,
    #[serde(default)] pub blocking: bool,
    /// 保留原生问题、schema 和允许的决定，响应由宿主逐项校验。
    #[serde(default)] pub data: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    #[serde(default)] pub parent_id: String,
    #[serde(default)] pub title: String,
    #[serde(default)] pub model: String,
    #[serde(default)] pub state: String,
    #[serde(default)] pub turn_id: String,
    #[serde(default)] pub message: String,
    #[serde(default)] pub kind: String,
    #[serde(default)] pub data: Value,
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
    pub queue: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub protocol_version: u32,
    pub runtime_id: String, pub session_id: String, pub tab_id: u32, pub generation: u32,
    pub agent: String, pub title: String, pub cwd: String,
    pub model: String, pub models: Vec<String>, pub policy: String, pub state: String,
    pub turn_id: String, pub seq: u64, pub settings_revision: u64,
    pub shared: bool, pub allow_control: bool,
    pub capabilities: Capabilities,
    pub items: Vec<Item>, pub approvals: Vec<Approval>,
    pub interactions: Vec<Interaction>, pub tasks: Vec<AgentTask>,
    pub permissions: Value, pub queued_inputs: Vec<Value>, pub activity: Value,
    pub turns: Vec<Value>, pub notices: Vec<Value>, pub metadata: Value, pub clock_ms: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub id: String, pub runtime_id: String, pub generation: u32, pub session_id: String,
    #[serde(default)] pub settings_revision: Option<u64>,
    #[serde(flatten)] pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Action {
    SendText { text: String },
    StopTurn { #[serde(rename = "turnId")] turn_id: String },
    SetModel { model: String },
    SetPolicy { sandbox: String, #[serde(rename = "approvalPolicy")] approval_policy: String },
    SetPermissions { profile: String, #[serde(rename = "approvalPolicy")] approval_policy: String, reviewer: String },
    EnterPlan,
    ContinueBlocked { #[serde(rename = "turnId")] turn_id: String, #[serde(rename = "reviewId")] review_id: String },
    RespondApproval { #[serde(rename = "requestId")] request_id: String, allow: bool },
    RespondInteraction { #[serde(rename = "requestId")] request_id: String, response: Value },
    Queue { operation: String, #[serde(default, rename = "queueId")] id: String, #[serde(default)] text: String, #[serde(default)] ids: Vec<String> },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt { pub id: String, pub status: String, pub message: Option<String> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    Ready { #[serde(rename = "sessionId")] session_id: String, model: String, models: Vec<String>, capabilities: Capabilities },
    State { state: String, #[serde(default, rename = "turnId")] turn_id: String },
    Items { items: Vec<Item> },
    TaskItems { #[serde(rename = "agentId")] agent_id: String, items: Vec<Item> },
    Upsert { item: Item },
    Delta { id: String, delta: String },
    Settings { model: Option<String>, policy: Option<String> },
    Permissions { value: Value },
    Queue { items: Vec<Value> },
    Activity { value: Value },
    Turn { value: Value },
    Notice { value: Value },
    Metadata { value: Value },
    Task { task: AgentTask },
    Interaction { interaction: Interaction },
    Resolved { id: String },
    Approval { approval: Approval },
    Result { id: String, ok: bool, message: Option<String> },
    Error { message: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch { pub session_id: String, pub events: Vec<Value> }

#[derive(Clone, Debug, Serialize)]
pub struct Change { pub seq: u64, pub events: Vec<Event> }
