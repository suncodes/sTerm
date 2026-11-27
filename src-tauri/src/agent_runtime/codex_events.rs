use super::protocol::{Event,Item};
use serde_json::{json,Value};

pub fn reasoning_text(value:&Value)->String {
    let summary=value["summary"].as_array().map(|parts|parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
    if !summary.is_empty(){summary}else{value["content"].as_array().map(|parts|parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n\n")).unwrap_or_default()}
}

pub fn tool_output(value:&Value)->String {
    let result=value.get("result").or(value.get("output")).unwrap_or(&Value::Null);
    if let Some(text)=result.as_str(){return text.into()}
    let content=result.as_array().or_else(||result.get("content").and_then(Value::as_array)).or_else(||value.get("contentItems").and_then(Value::as_array));
    let text=content.into_iter().flatten().filter_map(|part|part["text"].as_str()).collect::<Vec<_>>().join("\n");
    if text.is_empty(){value["error"]["message"].as_str().unwrap_or_default().into()}else{text}
}

pub fn incremental_item(method:&str,p:&Value,turn:&str,agent:&str,previous:Option<Item>)->Option<Item>{
    let reasoning=method.starts_with("item/reasoning/");
    if !reasoning && !["item/fileChange/patchUpdated","item/fileChange/outputDelta","item/mcpToolCall/progress","item/commandExecution/terminalInteraction"].contains(&method){return None}
    let native=p["itemId"].as_str()?;
    let mut item=previous.unwrap_or_else(||Item{id:if agent.is_empty(){native.into()}else{format!("{agent}:{native}")},kind:if reasoning{"thinking"}else{"tool"}.into(),title:if reasoning{"思考摘要"}else{"工具"}.into(),turn_id:turn.into(),agent_id:agent.into(),status:"running".into(),data:json!({}),..Default::default()});
    if !item.data.is_object(){item.data=json!({});}
    if reasoning {
        let field=if method=="item/reasoning/textDelta"{"content"}else{"summary"};
        let index=p[if field=="content"{"contentIndex"}else{"summaryIndex"}].as_u64().unwrap_or(0) as usize;
        if index>255{return None}
        if !item.data[field].is_array(){item.data[field]=json!([]);}
        let parts=item.data[field].as_array_mut()?;
        while parts.len()<=index{parts.push(json!(""));}
        if let Some(delta)=p["delta"].as_str(){let mut text=parts[index].as_str().unwrap_or_default().to_owned();if text.len()+delta.len()>48000{return None}text.push_str(delta);parts[index]=json!(text);}
        item.text=reasoning_text(&item.data);item.data["type"]=json!("reasoning");
    }else{match method{
        "item/fileChange/patchUpdated"=>{item.data["changes"]=p["changes"].clone();item.data["patch"]=p.get("patch").or(p.get("diff")).cloned().unwrap_or(Value::Null);},
        "item/fileChange/outputDelta"=>item.text.push_str(p["delta"].as_str().unwrap_or_default()),
        "item/mcpToolCall/progress"=>item.data["progress"]=p["message"].clone(),
        "item/commandExecution/terminalInteraction"=>item.data["stdin"]=p["stdin"].clone(),
        _=>{}
    }}
    Some(item)
}

pub fn passive_event(method:&str,p:&Value,agent:&str)->Option<Event>{
    let value=match method {
        "thread/name/updated"=>json!({"title":p.get("threadName").or(p.get("name"))}),
        "thread/tokenUsage/updated"=>json!({"tokenUsage":p["tokenUsage"]}),
        "turn/plan/updated"=>return Some(Event::Upsert{item:Item{id:format!("{agent}:plan-progress:{}",p["turnId"].as_str().unwrap_or_default()),kind:"plan".into(),title:"计划进度".into(),text:p["explanation"].as_str().unwrap_or_default().into(),turn_id:p["turnId"].as_str().unwrap_or_default().into(),agent_id:agent.into(),data:json!({"type":"planProgress","entries":p["plan"]}),..Default::default()}}),
        "turn/diff/updated"=>return Some(Event::Upsert{item:Item{id:format!("{agent}:turn-diff:{}",p["turnId"].as_str().unwrap_or_default()),kind:"tool".into(),title:"本轮文件变更汇总".into(),turn_id:p["turnId"].as_str().unwrap_or_default().into(),agent_id:agent.into(),data:json!({"type":"diff","diff":p["diff"]}),..Default::default()}}),
        "model/rerouted"=>json!({"actualModel":p["toModel"],"modelReason":p["reason"]}),
        "thread/environment/connected"|"thread/environment/disconnected"=>json!({"environment":{"id":p["environmentId"],"connected":method.ends_with("/connected")}}),
        "thread/goal/updated"=>json!({"goal":p["goal"]}),
        "thread/goal/cleared"=>json!({"goal":null}),
        "thread/archived"|"thread/deleted"=>return Some(Event::State{state:"offline".into(),turn_id:String::new()}),
        _=>return None,
    };
    if agent.is_empty(){Some(Event::Metadata{value})}else{None}
}

pub fn notice(method:&str,p:&Value,agent:&str)->Option<Value>{
    let title=match method{
        "warning"|"guardianWarning"=>"原生提示",
        "configWarning"=>"配置提示",
        "deprecationNotice"=>"兼容性提示",
        "model/rerouted"=>"本轮模型发生变化",
        "thread/compacted"=>"上下文已压缩",
        "thread/environment/connected"=>"运行环境已连接",
        "thread/environment/disconnected"=>"运行环境已断开",
        "hook/started"=>"正在执行 Hook",
        "hook/completed"=>"Hook 已结束",
        "item/autoApprovalReview/started"=>"正在自动审查",
        "item/autoApprovalReview/completed"=>"自动审查已结束",
        "autoApprovalReview/strictReviewRequired"=>"原生要求严格审查",
        "mcpServer/startupStatus/updated"=>"工具服务器连接状态",
        "mcpServer/oauthLogin/completed"=>"工具授权结果",
        "modelProvider/authRecoveryStarted"=>"正在恢复模型认证",
        "modelProvider/authRecoveryCompleted"=>"模型认证恢复已结束",
        "model/verification"=>"模型验证状态",
        "model/safetyBuffering/updated"=>"原生正在处理响应",
        "windows/worldWritableWarning"=>"Windows 工作目录提示",
        "windowsSandbox/setupCompleted"=>"Windows 沙箱准备结果",
        _=>return None,
    };
    let message=p["message"].as_str().or(p["summary"].as_str()).or(p["reason"].as_str()).or(p["review"]["rationale"].as_str()).or(p["run"]["statusMessage"].as_str()).or(p["error"].as_str()).or(p["status"].as_str()).unwrap_or(title);
    let mut text=message.chars().take(4000).collect::<String>();
    if method=="model/rerouted"{text=format!("{} → {} · {}",p["fromModel"].as_str().unwrap_or_default(),p["toModel"].as_str().unwrap_or_default(),message);}
    if let Some(details)=p["details"].as_str(){text.push('\n');text.extend(details.chars().take(4000));}
    let family=match method{
        "hook/started"|"hook/completed"=>"hook",
        "item/autoApprovalReview/started"|"item/autoApprovalReview/completed"=>"autoApprovalReview",
        "modelProvider/authRecoveryStarted"|"modelProvider/authRecoveryCompleted"=>"authRecovery",
        _=>method,
    };
    let state=p["review"]["status"].as_str().or(p["run"]["status"].as_str()).or(p["status"].as_str()).or(p["state"].as_str());
    let phase=if method.ends_with("/started")||method.ends_with("RecoveryStarted"){Some("running")}
        else if method.ends_with("/completed")||method.ends_with("RecoveryCompleted"){Some("completed")}
        else if ["mcpServer/startupStatus/updated","model/verification","model/safetyBuffering/updated"].contains(&method){
            match state{
                Some("running"|"inProgress"|"in_progress"|"pending"|"starting"|"retrying"|"verifying"|"buffering")=>Some("running"),
                Some("completed"|"succeeded"|"ready"|"failed"|"denied"|"cancelled"|"canceled"|"stopped"|"idle")=>Some("completed"),
                _=>None,
            }
        }else{None};
    let entity=p["reviewId"].as_str().or(p["run"]["id"].as_str()).or(p["itemId"].as_str()).or(p["server"].as_str()).or(p["turnId"].as_str()).unwrap_or_default();
    let severity=if matches!(state,Some("failed"|"error"))||p.get("error").is_some_and(|error|!error.is_null()){ "error" }
        else if method.contains("Warning")||method=="warning"||state==Some("denied"){"warning"}else{"info"};
    let title=if method=="model/safetyBuffering/updated"&&phase==Some("completed"){"原生响应处理已结束"}else{title};
    Some(json!({"id":format!("{agent}:{family}:{entity}"),"agentId":agent,"turnId":p["turnId"],"title":title,"message":text,"state":state,"phase":phase,"severity":severity,"updatedAtMs":super::presentation::now_ms()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notice_progress_updates_one_record_and_keeps_native_failure() {
        for (started,completed) in [
            ("hook/started","hook/completed"),
            ("item/autoApprovalReview/started","item/autoApprovalReview/completed"),
            ("modelProvider/authRecoveryStarted","modelProvider/authRecoveryCompleted"),
        ]{
            let start=notice(started,&json!({"turnId":"turn","itemId":"item","reviewId":"review","run":{"id":"run","status":"running"}}),"child").unwrap();
            let end=notice(completed,&json!({"turnId":"turn","itemId":"item","reviewId":"review","run":{"id":"run","status":"failed"}}),"child").unwrap();
            assert_eq!(start["id"],end["id"]);assert_eq!(start["phase"],"running");
            assert_eq!(end["phase"],"completed");assert_eq!(end["severity"],"error");
            assert_eq!(start["agentId"],"child");assert_eq!(start["turnId"],"turn");
        }
        let start=notice("mcpServer/startupStatus/updated",&json!({"server":"tool","status":"starting"}),"").unwrap();
        let end=notice("mcpServer/startupStatus/updated",&json!({"server":"tool","status":"ready"}),"").unwrap();
        assert_eq!(start["id"],end["id"]);assert_eq!(start["phase"],"running");assert_eq!(end["phase"],"completed");
        assert!(notice("warning",&json!({"message":"Read this"}),"").unwrap()["phase"].is_null());
    }
    #[test]
    fn indexed_reasoning_prefers_summary_and_retains_raw_blocks() {
        let raw=incremental_item("item/reasoning/textDelta",&json!({"itemId":"r","contentIndex":0,"delta":"raw"}),"turn","child",None).unwrap();
        let summary=incremental_item("item/reasoning/summaryTextDelta",&json!({"itemId":"r","summaryIndex":0,"delta":"summary"}),"turn","child",Some(raw)).unwrap();
        let next=incremental_item("item/reasoning/summaryTextDelta",&json!({"itemId":"r","summaryIndex":1,"delta":"second"}),"turn","child",Some(summary)).unwrap();
        assert_eq!(next.id,"child:r");assert_eq!(next.text,"summary\n\nsecond");assert_eq!(next.data["content"][0],"raw");
        assert!(incremental_item("item/reasoning/summaryTextDelta",&json!({"itemId":"r","summaryIndex":65535,"delta":"bad"}),"turn","",None).is_none());
    }
}
