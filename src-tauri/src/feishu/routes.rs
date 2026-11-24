use serde::Deserialize;
use std::{collections::HashMap, time::{Duration, Instant}};

use super::{valid_id, Config};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct SessionKey {
    pub id: u32,
    pub generation: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Topic {
    pub root: String,
    pub thread: Option<String>,
}

#[derive(Deserialize)]
struct Event {
    header: EventHeader,
    event: EventBody,
}

#[derive(Deserialize)]
struct EventHeader {
    event_type: String,
    app_id: String,
}

#[derive(Deserialize)]
struct EventBody {
    sender: Sender,
    message: Message,
}

#[derive(Deserialize)]
struct Sender {
    sender_type: String,
    sender_id: UserId,
}

#[derive(Deserialize)]
struct UserId {
    open_id: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    content: String,
    #[serde(default)]
    mentions: Vec<Mention>,
    root_id: Option<String>,
    parent_id: Option<String>,
    thread_id: Option<String>,
}

#[derive(Deserialize)]
struct Mention {
    key: String,
    id: UserId,
}

pub(super) struct Incoming {
    pub(crate) id: String,
    pub(crate) text: Option<String>,
    root: Option<String>,
    parent: Option<String>,
    thread: Option<String>,
}

impl Incoming {
    pub fn parse(payload: &[u8], config: &Config, bot: &str) -> Option<Self> {
        let event: Event = serde_json::from_slice(payload).ok()?;
        let EventBody { sender, message } = event.event;
        if event.header.app_id != config.app_id || event.header.event_type != "im.message.receive_v1"
            || message.chat_id != config.chat_id || message.chat_type != "group"
            || sender.sender_type != "user" || !valid_id(&message.message_id, "om_")
            || !config.allows_sender(sender.sender_id.open_id.as_deref())
        {
            return None;
        }
        let mentions: Vec<_> = message.mentions.iter().filter(|mention| mention.id.open_id.as_deref() == Some(bot)).collect();
        if mentions.is_empty() { return None; }

        #[derive(Deserialize)]
        struct Text { text: String }
        let text = (message.message_type == "text").then(|| serde_json::from_str::<Text>(&message.content).ok()).flatten()
            .and_then(|mut body| {
                for mention in mentions {
                    if !mention.key.is_empty() { body.text = body.text.replace(&mention.key, ""); }
                }
                crate::agent_turn::plain_agent_text(&body.text)
            });
        Some(Self {
            id: message.message_id, text,
            root: message.root_id.filter(|id| valid_id(id, "om_")),
            parent: message.parent_id.filter(|id| valid_id(id, "om_")),
            thread: message.thread_id.filter(|id| valid_id(id, "omt_")),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Route {
    Session(SessionKey),
    Unbound,
    Expired,
}

#[derive(Default)]
pub(super) struct Routes {
    topics: HashMap<SessionKey, Topic>,
    retired: HashMap<String, Instant>,
    seen: HashMap<String, Instant>,
}

impl Routes {
    pub fn topic(&self, key: SessionKey) -> Option<Topic> { self.topics.get(&key).cloned() }

    pub fn bind(&mut self, key: SessionKey, topic: Topic) { self.topics.insert(key, topic); }

    pub fn retire(&mut self, key: SessionKey) {
        if let Some(topic) = self.topics.remove(&key) {
            self.retired.retain(|_, expires| *expires > Instant::now());
            let expires = Instant::now() + Duration::from_secs(24 * 60 * 60);
            self.retired.insert(topic.root, expires);
            if let Some(thread) = topic.thread { self.retired.insert(thread, expires); }
        }
    }

    pub fn accept(&mut self, id: &str) -> bool {
        self.seen.retain(|_, expires| *expires > Instant::now());
        if self.seen.len() >= 8192 || self.seen.contains_key(id) { return false; }
        self.seen.insert(id.to_owned(), Instant::now() + Duration::from_secs(24 * 60 * 60));
        true
    }

    pub fn target(&self, input: &Incoming) -> Route {
        let root = input.root.as_deref();
        let thread = input.thread.as_deref();
        let parent = input.parent.as_deref();
        let mut matched = None;
        for (&key, topic) in &self.topics {
            if root == Some(topic.root.as_str()) || parent == Some(topic.root.as_str())
                || (thread.is_some() && thread == topic.thread.as_deref())
            {
                if matched.is_some() && matched != Some(key) {
                    return Route::Unbound;
                }
                matched = Some(key);
            }
        }
        if [root, parent, thread].into_iter().flatten().any(|id| self.retired.get(id).is_some_and(|expires| *expires > Instant::now())) {
            return Route::Expired;
        }
        matched.map_or(Route::Unbound, Route::Session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(message_id: &str, root: Option<&str>, thread: Option<&str>, user: &str, text: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "header": {"app_id": "cli_app", "event_type": "im.message.receive_v1"},
            "event": {
                "sender":{"sender_type":"user", "sender_id":{"open_id":user}},
                "message": {"message_id":message_id, "root_id":root, "thread_id":thread,
                    "chat_id":"oc_group", "chat_type":"group", "message_type":"text",
                    "mentions":[{"key":"@_user_1","id":{"open_id":"ou_bot"}}],
                    "content":json!({"text":text}).to_string()
                }
            }
        })).unwrap()
    }

    #[test]
    fn follow_up_requires_an_explicit_opt_in_for_all_group_users() {
        let legacy = json!({"appId": "cli_app", "chatId": "oc_group", "allowedUsers": []});
        let mut config: Config = serde_json::from_value(legacy).unwrap();
        let payload = event("om_input", Some("om_first"), None, "ou_member", "@_user_1 继续");
        assert!(Incoming::parse(&payload, &config, "ou_bot").is_none());

        config.allow_all_users = true;
        assert!(Incoming::parse(&payload, &config, "ou_bot").is_some());
        assert!(Incoming::parse(&payload, &config, "ou_other").is_none());

        let original: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        for (field, value) in [
            ("/header/app_id", "cli_other"),
            ("/event/message/chat_id", "oc_other"),
            ("/event/message/chat_type", "p2p"),
            ("/event/sender/sender_type", "app"),
        ] {
            let mut event = original.clone();
            *event.pointer_mut(field).unwrap() = json!(value);
            let payload = serde_json::to_vec(&event).unwrap();
            assert!(Incoming::parse(&payload, &config, "ou_bot").is_none(), "{field}");
        }

        config.allowed_users = vec!["ou_owner".into(), "ou_colleague".into()];
        assert!(Incoming::parse(&payload, &config, "ou_bot").is_none());
        for user in ["ou_owner", "ou_colleague"] {
            let payload = event("om_listed", Some("om_first"), None, user, "@_user_1 继续");
            assert!(Incoming::parse(&payload, &config, "ou_bot").is_some(), "{user}");
        }
    }

    #[test]
    fn routes_each_topic_and_rejects_unauthorized_duplicate_or_expired_messages() {
        let config = Config {
            app_id: "cli_app".into(), chat_id: "oc_group".into(), allowed_users: vec!["ou_owner".into()], ..Config::default()
        };
        let mut routes = Routes::default();
        let a = SessionKey { id: 1, generation: 1 };
        let b = SessionKey { id: 2, generation: 1 };
        routes.bind(a, Topic { root: "om_first".into(), thread: None });
        routes.bind(b, Topic { root: "om_second".into(), thread: Some("omt_second".into()) });

        let first = Incoming::parse(&event("om_input", Some("om_first"), None, "ou_owner", "@_user_1 继续"), &config, "ou_bot").unwrap();
        assert_eq!(first.text.as_deref(), Some("继续"));
        assert_eq!(routes.target(&first), Route::Session(a));
        let mut quoted = Incoming::parse(&event("om_quoted", Some("om_unbound_root"), None, "ou_owner", "@_user_1 继续"), &config, "ou_bot").unwrap();
        quoted.parent = Some("om_first".into());
        assert_eq!(routes.target(&quoted), Route::Session(a));
        quoted.root = Some("om_second".into());
        assert_eq!(routes.target(&quoted), Route::Unbound);
        assert!(routes.accept(&first.id));
        assert!(!routes.accept(&first.id));

        let second_event = event("om_other", None, Some("omt_second"), "ou_owner", "@_user_1 /model");
        let second = Incoming::parse(&second_event, &config, "ou_bot").unwrap();
        assert_eq!(routes.target(&second), Route::Session(b));
        assert!(second.text.is_none());
        assert!(Incoming::parse(&second_event, &config, "ou_other").is_none());
        assert!(Incoming::parse(&event("om_unknown", None, None, "ou_other", "@_user_1 继续"), &config, "ou_bot").is_none());

        routes.retire(a);
        routes.bind(SessionKey { id: 1, generation: 2 }, Topic { root: "om_restart".into(), thread: None });
        assert_eq!(routes.target(&first), Route::Expired);
        assert_eq!(routes.target(&second), Route::Session(b));
        let unbound = Incoming::parse(&event("om_direct", None, None, "ou_owner", "@_user_1 继续"), &config, "ou_bot").unwrap();
        assert_eq!(routes.target(&unbound), Route::Unbound);
    }
}
