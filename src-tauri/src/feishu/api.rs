//! 飞书应用鉴权与消息投递。API 错误只返回状态和业务码。
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use super::Config;

const ROOT: &str = "https://open.feishu.cn";

#[derive(Debug)]
pub(super) struct ApiError {
    operation: &'static str,
    status: Option<StatusCode>,
    code: Option<i64>,
    transient: bool,
}

impl ApiError {
    fn transport(operation: &'static str, error: reqwest::Error) -> Self {
        Self { operation, status: error.status(), code: None, transient: error.is_timeout() || error.is_connect() || error.is_body() }
    }

    fn invalid(operation: &'static str) -> Self {
        Self { operation, status: None, code: None, transient: false }
    }

    pub fn retryable(&self) -> bool { self.transient }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "飞书{}失败", self.operation)?;
        if let Some(status) = self.status { write!(f, "（HTTP {}", status.as_u16())?; }
        if let Some(code) = self.code { write!(f, "，错误码 {code}")?; }
        if self.status.is_some() { write!(f, "）")?; }
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct Endpoint {
    #[serde(rename = "URL")]
    pub url: String,
    #[serde(rename = "ClientConfig")]
    pub config: Option<SocketConfig>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(super) struct SocketConfig {
    pub ping_interval: u64,
    pub reconnect_interval: u64,
}

impl Default for SocketConfig {
    fn default() -> Self { Self { ping_interval: 120, reconnect_interval: 30 } }
}

#[derive(Deserialize)]
struct Token {
    tenant_access_token: String,
    expire: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct Sent {
    pub message_id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Identity {
    pub open_id: String,
    pub name: String,
}

pub(super) struct Api {
    client: Client,
    config: Config,
    token: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl Api {
    pub fn new(config: Config) -> Result<Self, String> {
        let client = Client::builder().https_only(true).redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8)).timeout(Duration::from_secs(12))
            .user_agent("sTerm").build()
            .map_err(|_| "无法初始化飞书连接")?;
        Ok(Self { client, config, token: tokio::sync::Mutex::new(None) })
    }

    pub async fn endpoint(&self) -> Result<Endpoint, ApiError> {
        let operation = "长连接鉴权";
        let response = self.client.post(format!("{ROOT}/callback/ws/endpoint")).header("locale", "zh")
            .json(&json!({"AppID": self.config.app_id, "AppSecret": self.config.app_secret}))
            .send().await.map_err(|error| ApiError::transport(operation, error))?;
        let mut value = checked(response, operation).await?;
        serde_json::from_value(value["data"].take()).map_err(|_| ApiError::invalid(operation))
    }

    async fn tenant_token(&self, refresh: Option<&str>) -> Result<String, ApiError> {
        let mut cache = self.token.lock().await;
        if let Some((token, expiration)) = cache.as_ref() {
            if *expiration > Instant::now() && refresh != Some(token.as_str()) { return Ok(token.clone()); }
        }
        let operation = "应用鉴权";
        let response = self.client.post(format!("{ROOT}/open-apis/auth/v3/tenant_access_token/internal"))
            .json(&json!({"app_id": self.config.app_id, "app_secret": self.config.app_secret}))
            .send().await.map_err(|error| ApiError::transport(operation, error))?;
        let value = checked(response, operation).await?;
        let token: Token = serde_json::from_value(value).map_err(|_| ApiError::invalid(operation))?;
        if token.tenant_access_token.is_empty() { return Err(ApiError::invalid(operation)); }
        let expiration = Instant::now() + Duration::from_secs(token.expire.min(86400).saturating_sub(60));
        *cache = Some((token.tenant_access_token.clone(), expiration));
        Ok(token.tenant_access_token)
    }

    async fn request(&self, operation: &'static str, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        let mut token = self.tenant_token(None).await?;
        for attempt in 0..2 {
            let url = format!("{ROOT}{path}");
            let request = match body {
                Some(body) => self.client.post(url).json(body),
                None => self.client.get(url),
            };
            let response = request.bearer_auth(&token).send().await
                .map_err(|error| ApiError::transport(operation, error))?;
            match checked(response, operation).await {
                Err(error) if attempt == 0 && (error.status == Some(StatusCode::UNAUTHORIZED)
                    || matches!(error.code, Some(99991661 | 99991663 | 99991664 | 99991668))) => {
                    token = self.tenant_token(Some(&token)).await?;
                }
                result => return result,
            }
        }
        unreachable!()
    }

    pub async fn identity(&self) -> Result<Identity, ApiError> {
        #[derive(Deserialize)]
        struct Bot { open_id: String, #[serde(default)] app_name: String }
        let operation = "读取机器人身份";
        let mut value = self.request(operation, "/open-apis/bot/v3/info", None).await?;
        let bot: Bot = serde_json::from_value(value["bot"].take()).map_err(|_| ApiError::invalid(operation))?;
        if !super::valid_id(&bot.open_id, "ou_") { return Err(ApiError::invalid(operation)); }
        Ok(Identity { open_id: bot.open_id, name: bot.app_name })
    }

    pub async fn send(&self, root: Option<&str>, kind: &str, content: &Value, uuid: &str) -> Result<Sent, ApiError> {
        let mut body = json!({"msg_type": kind, "content": content.to_string(), "uuid": uuid});
        let path = if let Some(root) = root {
            body["reply_in_thread"] = json!(true);
            format!("/open-apis/im/v1/messages/{root}/reply")
        } else {
            body["receive_id"] = json!(self.config.chat_id);
            "/open-apis/im/v1/messages?receive_id_type=chat_id".to_owned()
        };
        let operation = "消息投递";
        let mut value = self.request(operation, &path, Some(&body)).await?;
        let sent: Sent = serde_json::from_value(value["data"].take()).map_err(|_| ApiError::invalid(operation))?;
        if !super::valid_id(&sent.message_id, "om_") { return Err(ApiError::invalid(operation)); }
        Ok(sent)
    }
}

async fn checked(response: Response, operation: &'static str) -> Result<Value, ApiError> {
    let status = response.status();
    let body: Value = response.json().await.map_err(|error| {
        let mut result = ApiError::transport(operation, error);
        result.status = Some(status);
        result.transient |= status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
        result
    })?;
    let code = body.get("code").and_then(Value::as_i64);
    if status.is_success() && code == Some(0) { return Ok(body); }
    Err(ApiError {
        operation, status: Some(status), code,
        transient: status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || matches!(code, Some(1 | 99991400 | 99991401)),
    })
}
