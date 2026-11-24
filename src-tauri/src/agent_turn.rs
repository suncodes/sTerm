use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    Start,
    Prompt,
    Stop,
    Ready,
    Complete,
    Blocked,
    Unblocked,
    Error,
    End,
}

#[derive(Debug)]
pub struct AgentInput {
    /// 正文不含最后的提交回车。
    pub data: String,
    pub submit_delay: Duration,
}

#[derive(Default)]
enum InputState {
    #[default]
    Unavailable,
    Working,
    Blocked,
    Completed { revision: u64, paste: bool },
    Changed,
}

#[derive(Default)]
pub struct AgentTurn {
    agent: String,
    started: bool,
    input: InputState,
}

impl AgentTurn {
    /// 返回本轮是否刚刚完成。重复状态、等待和异常退出不产生完成事件。
    pub fn report(&mut self, agent: &str, event: AgentEvent, paste: bool, observed: Option<u64>, revision: u64) -> bool {
        if self.agent != agent || matches!(event, AgentEvent::Start) {
            *self = Self { agent: agent.to_owned(), ..Self::default() };
        }
        match event {
            AgentEvent::Prompt => {
                self.started = true;
                self.input = InputState::Working;
            }
            AgentEvent::Blocked => self.input = InputState::Blocked,
            AgentEvent::Unblocked if matches!(self.input, InputState::Blocked) => {
                self.input = if self.started { InputState::Working } else { InputState::Unavailable };
            }
            AgentEvent::Complete if self.started => {
                self.started = false;
                self.input = if observed == Some(revision) {
                    InputState::Completed { revision, paste }
                } else {
                    InputState::Changed
                };
                return true;
            }
            AgentEvent::Error | AgentEvent::End => *self = Self::default(),
            AgentEvent::Ready if matches!(self.input, InputState::Blocked | InputState::Working) => {
                self.input = InputState::Unavailable;
            }
            _ => {}
        }
        false
    }

    pub fn take_input(&mut self, revision: u64, text: &str) -> Result<AgentInput, &'static str> {
        let (expected, paste) = match self.input {
            InputState::Completed { revision, paste } => (revision, paste),
            InputState::Working => return Err("Agent 正在运行，请等本轮完成"),
            InputState::Blocked => return Err("Agent 正在等待操作，请打开远程终端"),
            InputState::Changed => return Err("终端已有其他输入，请打开远程终端"),
            InputState::Unavailable => return Err("原 Agent 未就绪，请打开远程终端"),
        };
        if expected != revision {
            self.input = InputState::Changed;
            return Err("终端已有其他输入，请打开远程终端");
        }
        if text.contains('\n') && !paste {
            return Err("此终端当前仅支持单行续问，请在远程终端发送多行内容");
        }
        let codex = self.agent == "codex";
        let data = if codex && paste {
            // 每行单独粘贴，段间使用明确的 Alt+Enter 换行，兼容 Windows 的逐键输入。
            text.split('\n').map(|line| format!("\x1b[200~{line}\x1b[201~")).collect::<Vec<_>>().join("\x1b\r")
        } else {
            let text = text.replace('\n', "\r");
            if paste { format!("\x1b[200~{text}\x1b[201~") } else { text }
        };
        self.input = InputState::Working;
        Ok(AgentInput {
            data,
            // Codex 有 120ms 的防误提交窗口；长文本还需给 Windows 逐键处理预留时间。
            submit_delay: if codex { Duration::from_millis(300 + text.chars().count() as u64 / 256 * 100) } else { Duration::ZERO },
        })
    }

    pub fn invalidate_input(&mut self) {
        if matches!(self.input, InputState::Completed { .. }) { self.input = InputState::Changed; }
    }

    pub fn reject_input(&mut self) {
        self.input = InputState::Unavailable;
    }
}

pub fn plain_agent_text(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let text = text.trim_matches(|c: char| c.is_whitespace() && !matches!(c, '\t' | '\r'));
    if text.is_empty() || text.len() > 8000 || text.starts_with(['/', '!'])
        || text.chars().any(|c| c != '\n' && c.is_control())
    {
        return None;
    }
    Some(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_explicit_completion_and_consumes_each_input_slot_once() {
        let mut turn = AgentTurn::default();
        assert!(!turn.report("codex", AgentEvent::Complete, true, Some(0), 0));
        turn.report("codex", AgentEvent::Prompt, true, None, 0);
        turn.report("codex", AgentEvent::Blocked, true, None, 0);
        assert!(turn.take_input(0, "继续").unwrap_err().contains("等待"));
        assert!(turn.report("codex", AgentEvent::Complete, true, Some(0), 0));
        assert!(!turn.report("codex", AgentEvent::Complete, true, Some(0), 0));
        assert_eq!(turn.take_input(0, "继续\n第二行").unwrap().data, "\x1b[200~继续\x1b[201~\x1b\r\x1b[200~第二行\x1b[201~");
        assert!(turn.take_input(0, "重复").is_err());
        turn.report("codex", AgentEvent::Prompt, false, None, 1);
        assert!(turn.report("codex", AgentEvent::Complete, false, Some(1), 1));
        assert!(turn.take_input(1, "多\n行").is_err());
        assert_eq!(turn.take_input(1, "单行").unwrap().data, "单行");
    }

    #[test]
    fn codex_preserves_paragraphs_and_separates_submission_from_paste() {
        for agent in ["codex", "claude", "pi"] {
            let mut turn = AgentTurn::default();
            turn.report(agent, AgentEvent::Prompt, true, None, 7);
            assert!(turn.report(agent, AgentEvent::Complete, true, Some(7), 7));
            let input = turn.take_input(7, "第一段\n\n第三段").unwrap();
            let (expected, delay) = if agent == "codex" {
                ("\x1b[200~第一段\x1b[201~\x1b\r\x1b[200~\x1b[201~\x1b\r\x1b[200~第三段\x1b[201~", Duration::from_millis(300))
            } else {
                ("\x1b[200~第一段\r\r第三段\x1b[201~", Duration::ZERO)
            };
            assert_eq!(input.data, expected, "{agent}");
            assert_eq!(input.submit_delay, delay, "{agent}");
            assert!(turn.take_input(7, "重复").is_err());
        }
    }

    #[test]
    fn preserves_legacy_pi_status_without_treating_it_as_completion() {
        let mut turn = AgentTurn::default();
        turn.report("pi", AgentEvent::Prompt, true, None, 0);
        assert!(!turn.report("pi", AgentEvent::Stop, true, None, 0));
        assert!(turn.take_input(0, "继续").is_err());
        assert!(turn.report("pi", AgentEvent::Complete, true, Some(0), 0));
        turn.report("pi", AgentEvent::End, true, None, 0);
        assert!(turn.take_input(0, "继续").is_err());
        turn.report("pi", AgentEvent::Prompt, true, None, 0);
        turn.report("pi", AgentEvent::Error, true, None, 0);
        assert!(!turn.report("pi", AgentEvent::Complete, true, Some(0), 0));
    }

    #[test]
    fn rejects_intervening_keyboard_input_and_non_text() {
        let mut turn = AgentTurn::default();
        turn.report("claude", AgentEvent::Prompt, true, None, 0);
        assert!(turn.report("claude", AgentEvent::Complete, true, Some(0), 1));
        assert!(turn.take_input(1, "继续").is_err());
        turn.report("claude", AgentEvent::Prompt, true, None, 1);
        assert!(turn.report("claude", AgentEvent::Complete, true, Some(1), 1));
        assert!(turn.take_input(2, "继续").is_err());
        for value in [" /model", "!echo", "\x03", "a\tb", "a\rb", "a\x1b[A", "   "] {
            assert_eq!(plain_agent_text(value), None, "{value:?}");
        }
        assert_eq!(plain_agent_text(" 中文\r\n继续 "), Some("中文\n继续".into()));
    }
}
