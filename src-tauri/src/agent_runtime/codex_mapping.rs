//! Codex 原生事件到共享交互的映射，保留服务端允许的决定与问题身份。
use super::protocol::*;
use serde_json::{json, Value};

pub fn scoped_item(value:&Value,turn:&str,agent:&str)->Option<Item>{
    let mut item=super::codex::item(value,turn)?;
    if !agent.is_empty(){item.id=format!("{agent}:{}",item.id);item.agent_id=agent.into();}
    Some(item)
}

pub fn questions(value:&Value)->Vec<Value>{
    value.as_array().into_iter().flatten().enumerate().map(|(index,q)|json!({
        "id":q["id"].as_str().map(str::to_owned).unwrap_or_else(||index.to_string()),
        "title":q.get("question").or(q.get("title")).and_then(Value::as_str).unwrap_or_default(),
        "header":q["header"],"multiSelect":false,"secret":q["isSecret"],"other":q.get("isOther").cloned().unwrap_or(json!(true)),
        "options":q["options"].as_array().map(|options|options.iter().map(|o|if let Some(label)=o.as_str(){json!({"label":label})}else{o.clone()}).collect::<Vec<_>>())
    })).collect()
}

pub fn request(method:&str,id:&str,p:&Value,agent:&str)->Option<Interaction>{
    let (kind,title,data)=match method{
        "item/commandExecution/requestApproval"|"item/fileChange/requestApproval"=>{
            let decisions=p["availableDecisions"].as_array().cloned().unwrap_or_else(||vec![json!("accept"),json!("acceptForSession"),json!("decline"),json!("cancel")]);
            let choices=decisions.into_iter().map(|value|{
                let label=match value.as_str(){Some("accept")=>"批准一次",Some("acceptForSession")=>"本会话允许",Some("decline")=>"拒绝此操作",Some("cancel")=>"取消本次请求",_ if value.get("acceptWithExecpolicyAmendment").is_some()=>"允许并记住命令规则",_ if value.get("applyNetworkPolicyAmendment").is_some()=>"应用网络规则",_=>"原生决定"};
                json!({"label":label,"value":value})
            }).collect::<Vec<_>>();
            ("approval",if method.contains("fileChange"){"修改文件"}else if p["kind"]=="writeStdin"{"向运行中命令输入"}else{"执行命令"},json!({"decisions":choices,"context":p}))
        }
        "item/tool/requestUserInput"=>("question","需要你回答",json!({"questions":questions(&p["questions"]),"cancellable":true})),
        "item/permissions/requestApproval"=>("permissions","申请额外执行权限",json!({"permissions":p["permissions"]})),
        "mcpServer/elicitation/request"=>{
            match p["mode"].as_str().unwrap_or("form"){
                "form"|"openai/form"|"openaiForm"=>("form","工具需要补充信息",json!({"schema":p["requestedSchema"],"serverName":p["serverName"]})),
                "url"=>("url","工具需要外部授权",json!({"url":p["url"],"serverName":p["serverName"]})),
                _=>("external","请在电脑端完成验证",json!({})),
            }
        }
        "item/tool/call"=>("external","此工具需要原生客户端处理",json!({})),
        _=>return None,
    };
    Some(Interaction{id:id.into(),kind:kind.into(),title:title.into(),
        detail:p["command"].as_str().or(p["reason"].as_str()).or(p["message"].as_str()).unwrap_or_default().into(),
        turn_id:p["turnId"].as_str().unwrap_or_default().into(),agent_id:agent.into(),
        blocking:p["isBlocking"].as_bool().unwrap_or(true),data})
}

pub fn response(interaction:&Interaction,native:&Value,response:&Value)->Result<Value,String>{
    super::interactions::validate(interaction,response)?;
    Ok(match interaction.kind.as_str(){
        "approval"|"plan"=>json!({"decision":response["decision"]}),
        "question"=>{
            if response["cancel"]==true {json!({"answers":{}})}
            else{
                let answers=response["answers"].as_object().ok_or("缺少答案")?.iter().map(|(key,value)|(key.clone(),json!({"answers":value}))).collect::<serde_json::Map<_,_>>();
                json!({"answers":answers})
            }
        }
        "permissions"=>json!({"permissions":if response["scope"]=="deny"{json!({})}else{serde_json::to_value(native["permissions"].as_object().ok_or("权限请求无效")?.iter().filter(|(key,value)|["network","fileSystem"].contains(&key.as_str())&&!value.is_null()).map(|(k,v)|(k.clone(),v.clone())).collect::<serde_json::Map<_,_>>()).map_err(|_|"权限请求无效")?},"scope":if response["scope"]=="session"{"session"}else{"turn"}}),
        "form"|"url"=>json!({"action":response["action"],"content":response.get("content").cloned().unwrap_or(Value::Null),"_meta":Value::Null}),
        _=>return Err("请在电脑端完成此交互".into()),
    })
}

pub fn terminal_state(turn:&Value)->(&'static str,Value){
    let error=&turn["error"];
    if error["codexErrorInfo"]=="misalignmentPolicyViolation"||error["codex_error_info"]=="misalignment_policy_violation"{
        return ("blocked",json!({"kind":"blocked","message":error["message"],"details":error["misalignment"]}));
    }
    match turn["status"].as_str(){
        Some("failed")=>("failed",json!({"kind":"failed","message":error["message"],"code":error["codexErrorInfo"]})),
        Some("interrupted")=>("interrupted",json!({"kind":"interrupted","message":"当前回合已中断"})),
        _=>("idle",json!({})),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn approval_preserves_allowed_decisions_and_question_answers(){
        let p=json!({"threadId":"t","availableDecisions":["accept","cancel"]});
        let interaction=request("item/commandExecution/requestApproval","r",&p,"child").unwrap();
        assert!(response(&interaction,&p,&json!({"decision":"acceptForSession"})).is_err());
        assert_eq!(response(&interaction,&p,&json!({"decision":"cancel"})).unwrap()["decision"],"cancel");
        let p=json!({"questions":[{"id":"q","question":"Which?","options":[{"label":"A","description":"one"}]}],"isBlocking":true});
        let interaction=request("item/tool/requestUserInput","r",&p,"").unwrap();
        assert_eq!(response(&interaction,&p,&json!({"answers":{"q":["A"]}})).unwrap(),json!({"answers":{"q":{"answers":["A"]}}}));
    }
    #[test]fn failures_and_system_blocks_are_not_completed(){
        assert_eq!(terminal_state(&json!({"status":"failed","error":{"codexErrorInfo":"misalignmentPolicyViolation"}})).0,"blocked");
        assert_eq!(terminal_state(&json!({"status":"failed","error":{"message":"bad key"}})).0,"failed");
        assert_eq!(terminal_state(&json!({"status":"interrupted"})).0,"interrupted");
    }
}
pub fn permission_current(value:&Value)->Value{
    let sandbox=value.get("sandboxPolicy").or(value.get("sandbox")).unwrap_or(&Value::Null);
    let profile=value["activePermissionProfile"]["id"].as_str().unwrap_or(match sandbox["type"].as_str(){Some("readOnly")=>":read-only",Some("workspaceWrite")=>":workspace",Some("dangerFullAccess")=>":danger-full-access",_=>"custom"});
    json!({"profile":profile,"approvalPolicy":value["approvalPolicy"],"reviewer":value.get("approvalsReviewer").cloned().unwrap_or(json!("user")),"sandbox":sandbox})
}

pub fn permission_options(profiles:&[Value],requirements:&Value,auto:bool)->Vec<Value>{
    let allowed=|key:&str,v:&str|requirements[key].as_array().is_none_or(|a|a.iter().any(|x|x==v));
    let mut rows=vec![];
    for p in profiles{
        let Some(id)=p["id"].as_str() else{continue};
        let (label,policy,mode)=match id{
            ":read-only"=>("Read Only · 只读","on-request","read-only"),
            ":workspace"=>("Default · 工作区写入","on-request","workspace-write"),
            ":danger-full-access"=>("Full Access · 完全访问","never","danger-full-access"),
            _=>(id,"on-request",""),
        };
        let profile_allowed=requirements["allowedPermissionProfiles"].get(id).is_none_or(|v|v==true);
        let is_allowed=p["allowed"]==true&&profile_allowed&&allowed("allowedApprovalPolicies",policy)&&allowed("allowedApprovalsReviewers","user")&&(mode.is_empty()||allowed("allowedSandboxModes",mode));
        rows.push(json!({"profile":id,"label":label,"description":p["description"],"approvalPolicy":policy,"reviewer":"user","allowed":is_allowed}));
        if id==":workspace"&&auto&&p["allowed"]==true&&profile_allowed&&allowed("allowedApprovalPolicies","on-request")&&allowed("allowedApprovalsReviewers","auto_review")&&allowed("allowedSandboxModes",mode){
            rows.push(json!({"profile":id,"label":"Auto-review · 自动审查","approvalPolicy":"on-request","reviewer":"auto_review","allowed":true}));
        }
    }
    rows
}

pub fn async_reply(interaction:&Interaction,response:&Value)->Result<String,String>{
    super::interactions::validate(interaction,response)?;
    let q=interaction.data["questions"].as_array().and_then(|q|q.first()).ok_or("问题已失效")?;
    let id=q["id"].as_str().ok_or("问题没有标识")?;
    let title=q["title"].as_str().unwrap_or_default();
    let mut end=title.len().min(512);while !title.is_char_boundary(end){end-=1;}
    let answer=response["answers"][id][0].as_str().ok_or("缺少答案")?;
    let body=json!([{"answer":answer,"question":title[..end].replace(['\n','\r']," "),"questionItemId":id}]);
    Ok(format!("<send_user_message_question_reply>\n{body}\n</send_user_message_question_reply>"))
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    #[test]fn native_permission_requirements_and_async_answer_identity_are_preserved(){
        let options=permission_options(&[json!({"id":":read-only","allowed":true}),json!({"id":":workspace","allowed":true}),json!({"id":":danger-full-access","allowed":true}),json!({"id":"custom","allowed":true})],&json!({"allowedApprovalPolicies":["on-request"],"allowedPermissionProfiles":{"custom":false}}),true);
        assert!(options.iter().any(|p|p["profile"]==":workspace"&&p["reviewer"]=="auto_review"&&p["allowed"]==true));
        assert!(options.iter().any(|p|p["profile"]==":danger-full-access"&&p["allowed"]==false));
        assert!(options.iter().any(|p|p["profile"]=="custom"&&p["allowed"]==false));
        let id=json!(["request_user_input_async","item",0]).to_string();
        let interaction=Interaction{kind:"asyncQuestion".into(),data:json!({"questions":[{"id":id,"title":"A question"}]}),..Default::default()};
        let text=async_reply(&interaction,&json!({"answers":{id.clone():["B"]}})).unwrap();
        let body=text.strip_prefix("<send_user_message_question_reply>\n").unwrap().strip_suffix("\n</send_user_message_question_reply>").unwrap();
        let replies:Value=serde_json::from_str(body).unwrap();
        assert_eq!(replies[0]["questionItemId"],id);assert_eq!(replies[0]["answer"],"B");
    }
}
