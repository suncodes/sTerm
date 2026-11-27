//! 网页响应仅能回答当前原生请求提供的选项，不能构造任意上游 RPC。
use super::protocol::{Interaction, Session};
use serde_json::Value;

pub fn validate(interaction: &Interaction, response: &Value) -> Result<(), String> {
    if serde_json::to_vec(response).map_or(true, |v| v.len() > 64 * 1024) { return Err("回答过长".into()); }
    match interaction.kind.as_str() {
        "question" | "asyncQuestion" => {
            if response["cancel"] == true && interaction.data["cancellable"] == true { return Ok(()); }
            let answers = response["answers"].as_object().ok_or("请选择答案")?;
            let questions = interaction.data["questions"].as_array().ok_or("问题格式无效")?;
            if answers.len() != questions.len() { return Err("请回答全部问题".into()); }
            for question in questions {
                let id = question["id"].as_str().ok_or("问题没有标识")?;
                let values = answers.get(id).and_then(Value::as_array).ok_or("问题答案缺失")?;
                if values.is_empty() || values.len() > 32 || question["multiSelect"] != true && values.len() != 1
                    || values.iter().any(|v| v.as_str().is_none_or(|s| s.trim().is_empty() || s.len() > 8192)) {
                    return Err("请选择有效答案".into());
                }
            }
        }
        "approval" | "plan" => {
            if !interaction.data["decisions"].as_array().is_some_and(|options| options.iter().any(|option| option["value"] == response["decision"])) {
                return Err("原生请求不支持此决定".into());
            }
        }
        "permissions" => {
            if !["turn", "session", "deny"].contains(&response["scope"].as_str().unwrap_or_default()) { return Err("请选择授权范围".into()); }
        }
        "form" => {
            let action = response["action"].as_str().unwrap_or_default();
            if !["accept", "decline", "cancel"].contains(&action) { return Err("无效表单操作".into()); }
            if action == "accept" { validate_form(&interaction.data["schema"], &response["content"])?; }
        }
        "url" => {
            let url=interaction.data["url"].as_str().and_then(|s|tauri::Url::parse(s).ok()).ok_or("授权地址无效")?;
            if !["http","https"].contains(&url.scheme()){return Err("请在电脑端完成此授权".into())}
            if !["accept","decline","cancel"].contains(&response["action"].as_str().unwrap_or_default()) { return Err("无效授权操作".into()); }
        }
        _ => return Err("此交互需要在电脑端处理".into()),
    }
    Ok(())
}

/// MCP 带标题枚举使用 oneOf/anyOf；只转换规范定义的 const 选项，不解释任意联合 schema。
pub fn normalize_schema(schema:&Value)->Value{
    let mut result=schema.clone();
    if let Some(fields)=result["properties"].as_object_mut(){
        for field in fields.values_mut(){
            if let Some(options)=field["oneOf"].as_array().filter(|a|!a.is_empty()&&a.iter().all(|v|v["const"].is_string()&&v.as_object().is_some_and(|o|o.keys().all(|k|["const","title"].contains(&k.as_str()))))).cloned(){
                field["enum"]=serde_json::json!(options.iter().map(|v|v["const"].clone()).collect::<Vec<_>>());
                field["enumNames"]=serde_json::json!(options.iter().map(|v|v.get("title").unwrap_or(&v["const"]).clone()).collect::<Vec<_>>());
                field.as_object_mut().unwrap().remove("oneOf");
            }
            if let Some(options)=field["items"]["anyOf"].as_array().filter(|a|!a.is_empty()&&a.iter().all(|v|v["const"].is_string()&&v.as_object().is_some_and(|o|o.keys().all(|k|["const","title"].contains(&k.as_str()))))).cloned(){
                field["items"]=serde_json::json!({"type":"string","enum":options.iter().map(|v|v["const"].clone()).collect::<Vec<_>>(),"enumNames":options.iter().map(|v|v.get("title").unwrap_or(&v["const"]).clone()).collect::<Vec<_>>()});
            }
        }
    }
    result
}

/// 只接受网页实际能呈现的平面 MCP schema；复杂 schema 交回原生界面。
pub fn form_supported(schema:&Value)->bool{
    let schema=normalize_schema(schema);
    let Some(object)=schema.as_object() else{return false};
    if schema["type"]!="object" || object.keys().any(|k|!["type","title","description","properties","required","additionalProperties","$schema"].contains(&k.as_str())){return false}
    let Some(fields)=schema["properties"].as_object() else{return false};
    fields.len()<=64&&fields.values().all(|field|{
        let Some(object)=field.as_object() else{return false};
        if object.keys().any(|k|!["type","title","description","enum","enumNames","default","items","minLength","maxLength","minimum","maximum","minItems","maxItems","uniqueItems","format"].contains(&k.as_str())){return false}
        match field["type"].as_str(){
            Some("string")=>field["format"].as_str().is_none_or(|f|["email","uri","date","date-time"].contains(&f)),
            Some("number"|"integer"|"boolean")=>true,
            Some("array")=>field["items"]["type"]=="string"&&field["items"]["enum"].is_array(),
            _=>false,
        }
    })
}

pub fn validate_form(schema: &Value, content: &Value) -> Result<(), String> {
    let normalized=normalize_schema(schema);let schema=&normalized;
    if !form_supported(schema){return Err("不支持的表单，请在电脑端处理".into())}
    let fields = schema["properties"].as_object().unwrap();
    let content = content.as_object().ok_or("请填写表单")?;
    if content.keys().any(|key| !fields.contains_key(key)) { return Err("表单包含未知字段".into()); }
    for (key, field) in fields {
        let required = schema["required"].as_array().is_some_and(|keys| keys.iter().any(|v| v == key));
        let Some(value) = content.get(key) else { if required { return Err(format!("请填写 {key}")); } else { continue; } };
        let valid = match field["type"].as_str().unwrap_or_default() {
            "string" => value.as_str().is_some_and(|s| s.len() <= 8192 && (!required || !s.trim().is_empty())&&s.chars().count()>=field["minLength"].as_u64().unwrap_or(0) as usize&&s.chars().count()<=field["maxLength"].as_u64().unwrap_or(u64::MAX) as usize),
            "number"|"integer" => value.as_f64().is_some_and(|n|n.is_finite()&&(field["type"]!="integer"||n.fract()==0.0)&&n>=field["minimum"].as_f64().unwrap_or(f64::NEG_INFINITY)&&n<=field["maximum"].as_f64().unwrap_or(f64::INFINITY)),
            "boolean" => value.is_boolean(),
            "array" => value.as_array().is_some_and(|a|a.len()<=32&&a.len()>=field["minItems"].as_u64().unwrap_or(0) as usize&&a.len()<=field["maxItems"].as_u64().unwrap_or(32) as usize&&a.iter().enumerate().all(|(i,v)|v.is_string()&&field["items"]["enum"].as_array().is_some_and(|options|options.contains(v))&&(field["uniqueItems"]!=true||!a[..i].contains(v)))),
            _ => false,
        };
        if !valid { return Err(format!("{key} 的类型、范围或长度无效")); }
        if let Some(format)=field["format"].as_str(){
            let text=value.as_str().ok_or("字符串格式无效")?;
            let valid=match format{
                "email"=>text.split_once('@').is_some_and(|(local,host)|!local.is_empty()&&host.contains('.')&&!text.chars().any(char::is_whitespace)),
                "uri"=>tauri::Url::parse(text).is_ok(),
                "date"=>valid_date(text),
                "date-time"=>time::OffsetDateTime::parse(text,&time::format_description::well_known::Rfc3339).is_ok(),
                _=>false,
            };
            if !valid{return Err(format!("{key} 的格式无效"))}
        }
        if let Some(options) = field["enum"].as_array() { if !options.contains(value) { return Err(format!("{key} 不在允许选项中")); } }
    }
    Ok(())
}

fn valid_date(text:&str)->bool{
    let parts:Vec<_>=text.split('-').collect();
    if parts.len()!=3||parts[0].len()!=4||parts[1].len()!=2||parts[2].len()!=2{return false}
    let (Ok(year),Ok(month),Ok(day))=(parts[0].parse::<i32>(),parts[1].parse::<u8>(),parts[2].parse::<u8>())else{return false};
    time::Month::try_from(month).is_ok_and(|month|time::Date::from_calendar_date(year,month,day).is_ok())
}
pub fn validate_permissions(session: &Session, profile: &str, approval: &str, reviewer: &str) -> Result<(), String> {
    let options = session.permissions["options"].as_array().ok_or("权限目录尚未就绪")?;
    if !options.iter().any(|option| option["profile"] == profile && option["approvalPolicy"] == approval && option["reviewer"] == reviewer && option["allowed"] == true) {
        return Err("该权限组合不在原生允许的选项中".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn questions_and_decisions_cannot_be_replaced_by_arbitrary_responses() {
        let question = Interaction { kind:"question".into(), data:json!({"questions":[{"id":"a"},{"id":"b","multiSelect":true}]}), ..Default::default() };
        assert!(validate(&question,&json!({"answers":{"a":["A"],"b":["B","C"]}})).is_ok());
        assert!(validate(&question,&json!({"answers":{"a":["A"]}})).is_err());
        assert!(validate(&question,&json!({"answers":{"a":["A","B"],"b":["C"]}})).is_err());
        let approval = Interaction { kind:"approval".into(), data:json!({"decisions":[{"value":"accept"}]}), ..Default::default() };
        assert!(validate(&approval,&json!({"decision":"accept"})).is_ok());
        assert!(validate(&approval,&json!({"decision":"acceptForSession"})).is_err());
    }
}

#[cfg(test)]
mod form_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn mcp_titled_choices_ranges_and_unknown_schema_are_validated(){
        let schema=json!({"type":"object","required":["target","count","features"],"properties":{
            "target":{"type":"string","oneOf":[{"const":"prod","title":"Production"},{"const":"test","title":"Test"}]},
            "count":{"type":"integer","minimum":1,"maximum":5},
            "features":{"type":"array","items":{"anyOf":[{"const":"x","title":"X"},{"const":"y","title":"Y"}]},"uniqueItems":true},
            "date":{"type":"string","format":"date"}
        }});
        assert!(validate_form(&schema,&json!({"target":"test","count":2,"features":["x","y"],"date":"2026-11-27"})).is_ok());
        assert!(validate_form(&schema,&json!({"target":"test","count":0,"features":["x"]})).is_err());
        assert!(validate_form(&schema,&json!({"target":"test","count":2,"features":["unknown"]})).is_err());
        assert!(validate_form(&schema,&json!({"target":"test","count":2,"features":["x","x"]})).is_err());
        assert!(validate_form(&schema,&json!({"target":"test","count":2,"features":["x"],"date":"2026-02-31"})).is_err());
        assert!(!form_supported(&json!({"type":"object","properties":{"nested":{"type":"object","properties":{}}}})));
        assert!(!form_supported(&json!({"type":"object","properties":{"choice":{"type":"string","oneOf":[{"pattern":"secret"}]}}})));
        let url=Interaction{kind:"url".into(),data:json!({"url":"javascript:alert(1)"}),..Default::default()};
        assert!(validate(&url,&json!({"action":"accept"})).is_err());
    }
}
