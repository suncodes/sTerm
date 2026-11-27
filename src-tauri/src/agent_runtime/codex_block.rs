//! 安全阻断的说明与继续材料仅保存在实时内存中，不能从历史记录重建授权。
use serde_json::{json, Value};
use super::protocol::Session;

pub fn is_block(error: &Value) -> bool {
    error["codexErrorInfo"] == "misalignmentPolicyViolation"
        || error["codex_error_info"] == "misalignment_policy_violation"
}

fn bounded_text(value: &Value, limit: usize) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty() && text.len() <= limit)
}

fn details(error: &Value) -> Value {
    let value = &error["misalignment"];
    let field = |camel: &str, snake: &str| value.get(camel).or_else(|| value.get(snake)).unwrap_or(&Value::Null);
    json!({
        "errorType": bounded_text(field("errorType", "error_type"), 1024),
        "detailedExplanation": bounded_text(field("detailedExplanation", "detailed_explanation"), 64 * 1024),
        "steer": {"message": bounded_text(&value["steer"]["message"], 1024)},
        "reviewTarget": bounded_text(field("reviewTarget", "review_target"), 4096),
    })
}

pub fn activity(turn: &str, error: &Value, previous: &Value, live: bool) -> Value {
    let same_turn = previous["kind"] == "blocked" && previous["turnId"] == turn;
    if live && same_turn && error["misalignment"].is_null() { return previous.clone(); }
    let current_details = if live { details(error) } else { Value::Null };
    if live && same_turn && previous["details"] == current_details { return previous.clone(); }
    json!({
        "kind": "blocked", "turnId": turn,
        "message": error["message"].as_str().unwrap_or("Chat stopped as a precaution").chars().take(8192).collect::<String>(),
        "code": "misalignmentPolicyViolation", "details": current_details,
        "reviewId": if live && !turn.is_empty() { Some(uuid::Uuid::new_v4().to_string()) } else { None },
    })
}

pub fn continuation_message<'a>(activity: &'a Value, turn: &str, review: &str) -> Result<&'a str, String> {
    if turn.is_empty() || review.is_empty() || activity["kind"] != "blocked"
        || activity["turnId"] != turn || activity["reviewId"] != review {
        return Err("阻断信息已更新，请重新查看说明".into());
    }
    bounded_text(&activity["details"]["detailedExplanation"], 64 * 1024)
        .ok_or("原生未提供完整恢复说明，此会话不能在此继续")?;
    bounded_text(&activity["details"]["steer"]["message"], 1024)
        .ok_or_else(|| "原生未提供有效继续指令，此会话不能在此继续".into())
}

pub fn validate(session: &Session, turn: &str, review: &str) -> Result<(), String> {
    if session.turns.iter().rev().find(|entry| entry["agentId"].as_str().unwrap_or_default().is_empty()).is_some_and(|entry| entry["state"] == "inProgress") {
        return Err("原生仍在结束被阻断的回合，请稍后确认".into());
    }
    if session.agent != "codex" || session.state != "blocked" || !session.capabilities.send {
        return Err("当前会话没有可确认的安全阻断".into());
    }
    if session.turns.iter().rev().find(|entry| entry["agentId"].as_str().unwrap_or_default().is_empty())
        .is_some_and(|entry| entry["id"] != turn) {
        return Err("阻断回合已变化，请刷新后查看".into());
    }
    continuation_message(&session.activity, turn, review)?;
    if session.activity["continuationStatus"] == "submitted" {
        return Err("继续请求已经提交，请等待原生状态确认，不会重复发送".into());
    }
    Ok(())
}

pub fn params(thread: &str, activity: &Value, turn: &str, review: &str) -> Result<Value, String> {
    let message = continuation_message(activity, turn, review)?;
    Ok(json!({
        "threadId": thread,
        "input": [{"type": "text", "text": message, "text_elements": []}],
        "responsesapiClientMetadata": {"misalignment_override": json!({"timestamp": super::presentation::now_ms()}).to_string()},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn error() -> Value { json!({"codexErrorInfo":"misalignmentPolicyViolation","message":"Blocked","misalignment":{"detailedExplanation":"Review the detected action.","steer":{"message":"Continue only within the requested scope."},"reviewTarget":"opaque"}}) }
    #[test]
    fn duplicate_terminal_errors_preserve_live_review_but_withdrawals_invalidate_it() {
        let first = activity("turn", &error(), &Value::Null, true);
        let review = first["reviewId"].as_str().unwrap();
        let duplicate = activity("turn", &json!({"message":"Blocked"}), &first, true);
        assert_eq!(duplicate, first);
        assert_eq!(activity("turn", &error(), &first, true), first);
        let mut withdrawn = error(); withdrawn["misalignment"]["detailedExplanation"] = Value::Null;
        let next = activity("turn", &withdrawn, &first, true);
        assert!(continuation_message(&next, "turn", review).is_err());
        assert!(continuation_message(&next, "turn", next["reviewId"].as_str().unwrap()).is_err());
        let history = activity("turn", &error(), &first, false);
        assert!(history["details"].is_null()); assert!(history["reviewId"].is_null());
    }
    #[test]
    fn continuation_requires_both_native_materials_and_never_uses_an_opaque_target_alone() {
        let valid = activity("turn", &error(), &Value::Null, true);
        let review = valid["reviewId"].as_str().unwrap();
        let wire = params("thread", &valid, "turn", review).unwrap();
        assert_eq!(wire["input"][0]["text"], error()["misalignment"]["steer"]["message"]);
        assert!(wire["responsesapiClientMetadata"]["misalignment_override"].as_str().unwrap().contains("timestamp"));
        for missing in ["detailedExplanation", "steer"] {
            let mut value = error(); value["misalignment"][missing] = Value::Null;
            let block = activity("turn", &value, &Value::Null, true);
            assert!(params("thread", &block, "turn", block["reviewId"].as_str().unwrap()).is_err());
        }
        let mut oversized = error(); oversized["misalignment"]["steer"]["message"] = json!("中".repeat(342));
        let block = activity("turn", &oversized, &Value::Null, true);
        assert!(continuation_message(&block, "turn", block["reviewId"].as_str().unwrap()).is_err());
        assert!(params("thread", &valid, "other-turn", review).is_err());
    }
}
