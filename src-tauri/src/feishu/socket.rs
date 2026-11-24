//! 飞书长连接的 Protobuf 帧与消息确认。
//! 字段编号与心跳、确认格式参考飞书开放平台 Go SDK 的 ws 包。
use super::api::{Api, SocketConfig};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use std::{collections::HashMap, time::{Duration, Instant}};
use tokio_tungstenite::{connect_async_with_config, tungstenite::{Message, protocol::WebSocketConfig}};
use tokio::sync::mpsc;

const MAX_PAYLOAD: usize = 1024 * 1024;

#[derive(Clone, PartialEq, prost::Message)]
struct Header {
    #[prost(string, required, tag = "1")]
    key: String,
    #[prost(string, required, tag = "2")]
    value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Frame {
    #[prost(uint64, required, tag = "1")]
    seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    log_id: u64,
    #[prost(int32, required, tag = "3")]
    service: i32,
    #[prost(int32, required, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<Header>,
    #[prost(string, optional, tag = "6")]
    payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    log_id_new: Option<String>,
}

impl Frame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|header| header.key == key).map(|header| header.value.as_str())
    }

    fn ping(service: i32) -> Self {
        Self { service, headers: vec![Header { key: "type".into(), value: "ping".into() }], ..Self::default() }
    }

    fn ack(&mut self, code: u16) -> Message {
        self.headers.push(Header { key: "biz_rt".into(), value: "0".into() });
        self.payload = Some(format!(r#"{{"code":{code},"headers":null,"data":null}}"#).into_bytes());
        Message::Binary(self.encode_to_vec().into())
    }
}

#[derive(Default)]
struct Fragments {
    pending: HashMap<String, (Instant, Vec<Option<Vec<u8>>>)>,
}

impl Fragments {
    fn collect(&mut self, frame: &mut Frame) -> Result<Option<Vec<u8>>, ()> {
        let payload = frame.payload.take().unwrap_or_default();
        let count = number(frame.header("sum"), 1)?;
        let index = number(frame.header("seq"), 0)?;
        if count == 1 && index == 0 { return Ok(Some(payload)); }
        if !(2..=32).contains(&count) || index >= count { return Err(()); }
        let key = frame.header("message_id").filter(|key| !key.is_empty() && key.len() <= 200).ok_or(())?;
        self.pending.retain(|_, (start, _)| start.elapsed() < Duration::from_secs(5));
        if self.pending.len() >= 16 && !self.pending.contains_key(key) { return Err(()); }
        let (_, parts) = self.pending.entry(key.to_owned()).or_insert_with(|| (Instant::now(), vec![None; count]));
        if parts.len() != count { return Err(()); }
        parts[index] = Some(payload);
        if parts.iter().flatten().map(Vec::len).sum::<usize>() > MAX_PAYLOAD {
            self.pending.remove(key);
            return Err(());
        }
        if parts.iter().any(Option::is_none) { return Ok(None); }
        Ok(self.pending.remove(key).map(|(_, parts)| parts.into_iter().flatten().flatten().collect()))
    }
}

fn number(value: Option<&str>, default: usize) -> Result<usize, ()> {
    value.map_or(Ok(default), |value| value.parse().map_err(|_| ()))
}

pub(super) async fn run(
    api: &Api,
    events: &mpsc::Sender<Vec<u8>>,
    mut publish: impl FnMut(bool, Option<String>),
) {
    let mut retry = 2;
    loop {
        publish(false, None);
        let result: Result<(), String> = async {
            let endpoint = api.endpoint().await.map_err(|error| error.to_string())?;
            let url = reqwest::Url::parse(&endpoint.url).map_err(|_| "飞书返回的长连接地址无效")?;
            if url.scheme() != "wss" || url.host().is_none() || !url.username().is_empty() || url.password().is_some() {
                return Err("飞书返回的长连接地址无效".into());
            }
            let service = url.query_pairs().find(|(name, _)| name == "service_id")
                .and_then(|(_, value)| value.parse().ok()).ok_or("飞书长连接缺少服务标识")?;
            let socket_config = WebSocketConfig::default().max_message_size(Some(MAX_PAYLOAD))
                .max_frame_size(Some(MAX_PAYLOAD));
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(12),
                connect_async_with_config(endpoint.url, Some(socket_config), false)).await
                .map_err(|_| "连接飞书超时")?.map_err(|_| "飞书长连接握手失败")?;
            let mut config = endpoint.config.unwrap_or_default();
            retry = config.reconnect_interval.clamp(2, 30);
            publish(true, None);
            let mut ping = Instant::now();
            let mut seen = Instant::now();
            let mut fragments = Fragments::default();
            loop {
                tokio::select! {
                    incoming = socket.next() => {
                        let Some(Ok(message)) = incoming else { return Err("飞书长连接已断开".into()); };
                        seen = Instant::now();
                        match message {
                            Message::Binary(bytes) => {
                                let Ok(mut frame) = Frame::decode(bytes) else { continue };
                                if frame.method == 0 && frame.header("type") == Some("pong") {
                                    if let Some(next) = frame.payload.as_deref().and_then(|body| serde_json::from_slice::<SocketConfig>(body).ok()) {
                                        config = next;
                                        retry = config.reconnect_interval.clamp(2, 30);
                                    }
                                } else if frame.method == 1 {
                                    let code = match fragments.collect(&mut frame) {
                                        Ok(Some(payload)) => {
                                            if frame.header("type") == Some("event") && events.try_send(payload).is_err() { 500 } else { 200 }
                                        }
                                        Ok(None) => continue,
                                        Err(()) => 500,
                                    };
                                    tokio::time::timeout(Duration::from_secs(2), socket.send(frame.ack(code)))
                                        .await.map_err(|_| "飞书消息确认超时")?.map_err(|_| "飞书消息确认失败")?;
                                }
                            }
                            Message::Ping(_) => socket.flush().await.map_err(|_| "飞书心跳响应失败")?,
                            Message::Close(_) => return Err("飞书长连接已关闭".into()),
                            _ => {}
                        }
                    }
                    _ = tokio::time::sleep_until(ping.into()) => {
                        let interval = Duration::from_secs(config.ping_interval.clamp(5, 300));
                        if seen.elapsed() > interval.saturating_mul(2) + Duration::from_secs(5) {
                            return Err("飞书长连接心跳超时".into());
                        }
                        tokio::time::timeout(Duration::from_secs(8), socket.send(Message::Binary(Frame::ping(service).encode_to_vec().into())))
                            .await.map_err(|_| "飞书心跳发送超时")?.map_err(|_| "飞书心跳发送失败")?;
                        ping = Instant::now() + interval;
                    }
                }
            }
        }.await;
        let error: String = result.unwrap_err();
        publish(false, Some(error));
        tokio::time::sleep(Duration::from_secs(retry)).await;
        retry = (retry * 2).min(30);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_sdk_wire_tags_and_preserves_routing_headers_in_ack() {
        let bytes = Frame::ping(42).encode_to_vec();
        assert_eq!(bytes, [
            8, 0, 16, 0, 24, 42, 32, 0, 42, 12, 10, 4, b't', b'y', b'p', b'e', 18, 4, b'p', b'i', b'n', b'g',
        ]);
        let mut frame = Frame::decode(bytes.as_slice()).unwrap();
        frame.log_id_new = Some("trace".into());
        frame.seq_id = 10;
        let Message::Binary(ack) = frame.ack(200) else { panic!("应答必须为二进制") };
        let decoded = Frame::decode(ack).unwrap();
        assert_eq!(decoded.seq_id, 10);
        assert_eq!(decoded.log_id_new.as_deref(), Some("trace"));
        assert_eq!(decoded.header("type"), Some("ping"));
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&decoded.payload.unwrap()).unwrap()["code"], 200);
    }

    #[test]
    fn joins_out_of_order_fragments_and_rejects_invalid_indexes() {
        let mut fragments = Fragments::default();
        let make = |index: &str, text: &str| Frame {
            headers: [("message_id", "msg"), ("sum", "2"), ("seq", index)].into_iter()
                .map(|(key, value)| Header { key: key.into(), value: value.into() }).collect(),
            payload: Some(text.as_bytes().to_vec()), ..Frame::default()
        };
        assert_eq!(fragments.collect(&mut make("2", "无效")), Err(()));
        assert_eq!(fragments.collect(&mut make("1", "后半")), Ok(None));
        assert_eq!(fragments.collect(&mut make("0", "前半")), Ok(Some("前半后半".as_bytes().to_vec())));
    }
}
