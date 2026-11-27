use serde_json::{json, Value};
use super::protocol::{AgentTask, Item};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

pub fn merge(target: &mut Value, patch: &Value) {
    if let Some(fields) = patch.as_object() {
        if !target.is_object() { *target = json!({}); }
        for (key, value) in fields {
            if value.is_null() && (key.ends_with("AtMs") || key=="durationMs") { continue; }
            target[key] = value.clone();
        }
    }
}

pub fn upsert_value(rows: &mut Vec<Value>, value: Value, limit: usize) {
    if !value["id"].is_string() { return; }
    if let Some(old)=rows.iter_mut().find(|old|old["id"]==value["id"] && old["agentId"]==value["agentId"]) {
        merge(old,&value);
    } else {
        rows.push(value);
        if rows.len()>limit { rows.remove(0); }
    }
}

pub fn merge_item(old: &Item, mut next: Item) -> Item {
    let mut data = old.data.clone(); merge(&mut data, &next.data); next.data = data;
    next
}

pub fn merge_task(old: &AgentTask, mut next: AgentTask) -> AgentTask {
    let mut data = old.data.clone(); merge(&mut data, &next.data); next.data = data;
    if next.kind.is_empty() { next.kind = old.kind.clone(); }
    next
}

pub fn turn(value: &Value, agent: &str) -> Value {
    let mut error=value["error"].clone();
    if let Some(fields)=error.as_object_mut(){fields.remove("misalignment");}
    json!({"id":value["id"],"agentId":agent,"state":value["status"],
        "startedAtMs":value["startedAt"].as_f64().map(|time|(time*1000.0) as u64),
        "completedAtMs":value["completedAt"].as_f64().map(|time|(time*1000.0) as u64),
        "durationMs":value["durationMs"],"error":error,"timeSource":"native"})
}

pub fn command_task(item: &Item) -> Option<AgentTask> {
    let process = item.data["processId"].as_str()?;
    let running = ["running", "inProgress"].contains(&item.status.as_str());
    Some(AgentTask { id:format!("process:{}:{process}", item.agent_id), parent_id:item.agent_id.clone(),
        title:item.data["command"].as_str().unwrap_or("后台命令").chars().take(160).collect(),
        state:if running { "running" } else if item.data["exitCode"].as_i64().is_some_and(|code|code!=0) { "failed" } else { item.status.as_str() }.into(),
        turn_id:item.turn_id.clone(), kind:"shell".into(), data:json!({"itemId":item.id,"processId":process,
            "startedAtMs":item.data["startedAtMs"],"completedAtMs":item.data["completedAtMs"],
            "durationMs":item.data["durationMs"],"exitCode":item.data["exitCode"],"cwd":item.data["cwd"],"source":"commandExecution","backgroundConfirmed":false,"canStop":false}),
        ..Default::default() })
}
