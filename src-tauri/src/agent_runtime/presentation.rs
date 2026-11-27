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
    preserve_reasoning(old, &mut next);
    let mut data = old.data.clone(); merge(&mut data, &next.data); next.data = data;
    next
}

// 原生终态可能只有加密 reasoning；保留同一条目已公开的流式摘要。
fn preserve_reasoning(old: &Item, next: &mut Item) {
    if old.id == next.id && old.agent_id == next.agent_id && old.turn_id == next.turn_id
        && old.kind == "thinking" && next.kind == "thinking" && old.data["type"] == "reasoning" && next.data["type"] == "reasoning"
        && !old.text.trim().is_empty() && next.text.trim().is_empty() {
        next.text = old.text.clone();
        for key in ["summary", "content"] { if let Some(value) = old.data.get(key) { next.data[key] = value.clone(); } }
    }
}
pub fn preserve_reasoning_history(old: &[Item], mut items: Vec<Item>) -> Vec<Item> {
    for item in &mut items {
        if item.kind == "thinking" && item.data["type"] == "reasoning" && item.text.trim().is_empty() {
            if let Some(previous) = old.iter().find(|previous|previous.id == item.id && previous.agent_id == item.agent_id && previous.turn_id == item.turn_id) { preserve_reasoning(previous, item); }
        }
    }
    items
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


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_empty_reasoning_completion_and_history_keep_only_the_same_public_stream() {
        let old=Item{id:"thought".into(),kind:"thinking".into(),turn_id:"turn".into(),text:"Public summary".into(),status:"running".into(),data:json!({"type":"reasoning","summary":["Public summary"],"content":[]}),..Default::default()};
        let completed=Item{text:String::new(),status:"completed".into(),data:json!({"type":"reasoning","summary":[],"content":[]}),..old.clone()};
        let merged=merge_item(&old,completed.clone());
        assert_eq!(merged.text,"Public summary");assert_eq!(merged.status,"completed");assert_eq!(merged.data["summary"][0],"Public summary");
        let refreshed=preserve_reasoning_history(&[old.clone()],vec![completed.clone()]);
        assert_eq!(refreshed[0].text,"Public summary");
        for other in [
            Item{turn_id:"another".into(),..completed.clone()},
            Item{agent_id:"child".into(),..completed.clone()},
            Item{id:"another".into(),..completed.clone()},
            Item{data:json!({"type":"thinking"}),..completed.clone()},
        ]{assert!(merge_item(&old,other.clone()).text.is_empty());assert!(preserve_reasoning_history(&[old.clone()],vec![other])[0].text.is_empty());}
        let replacement=merge_item(&old,Item{text:"Final public summary".into(),..completed});
        assert_eq!(replacement.text,"Final public summary");
    }
}
