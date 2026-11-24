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

#[derive(Clone, Copy, Default)]
enum InputState {
    #[default]
    Unavailable,
    Available,
    Submitting,
    Working,
    Blocked,
}

#[derive(Default)]
pub struct AgentTurn {
    agent: String,
    started: bool,
    input: InputState,
    before_blocked: InputState,
}

impl AgentTurn {
    /// 输入能力跟随当前状态；只有明确的本轮完成才产生一次通知。
    pub fn report(&mut self, agent: &str, event: AgentEvent) -> bool {
        if self.agent != agent || matches!(event, AgentEvent::Start) {
            *self = Self { agent: agent.to_owned(), ..Self::default() };
        }
        match event {
            AgentEvent::Start | AgentEvent::Ready => self.input = InputState::Available,
            AgentEvent::Prompt => {
                self.started = true;
                self.input = InputState::Working;
            }
            AgentEvent::Blocked => {
                if !matches!(self.input, InputState::Blocked) { self.before_blocked = self.input; }
                self.input = InputState::Blocked;
            }
            AgentEvent::Unblocked if matches!(self.input, InputState::Blocked) => {
                self.input = std::mem::take(&mut self.before_blocked);
            }
            AgentEvent::Complete => {
                let completed = std::mem::take(&mut self.started);
                if completed || !matches!(self.input, InputState::Submitting) {
                    self.input = InputState::Available;
                }
                return completed;
            }
            AgentEvent::Error | AgentEvent::End => *self = Self::default(),
            _ => {}
        }
        false
    }

    pub fn take_input(&mut self, text: &str, paste: bool) -> Result<AgentInput, &'static str> {
        match self.input {
            InputState::Available => {}
            InputState::Submitting => return Err("输入正在提交，请稍后重试"),
            InputState::Working => return Err("Agent 正在运行，请等本轮完成"),
            InputState::Blocked => return Err("Agent 正在等待操作，请打开远程终端"),
            InputState::Unavailable => return Err("原 Agent 未就绪，请打开远程终端"),
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
        self.started = false;
        self.input = InputState::Submitting;
        Ok(AgentInput {
            data,
            // Codex 有 120ms 的防误提交窗口；长文本还需给 Windows 逐键处理预留时间。
            submit_delay: if codex { Duration::from_millis(300 + text.chars().count() as u64 / 256 * 100) } else { Duration::ZERO },
        })
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
    fn explicit_readiness_allows_input_without_a_previous_completed_task() {
        for agent in ["codex", "claude", "pi"] {
            for event in [AgentEvent::Start, AgentEvent::Ready, AgentEvent::Complete] {
                let mut turn = AgentTurn::default();
                assert!(turn.take_input("继续", true).is_err());
                assert!(!turn.report(agent, event));
                assert!(turn.take_input("继续", true).is_ok(), "{agent}");
                assert!(turn.take_input("重复", true).unwrap_err().contains("提交"));
            }
        }
    }

    #[test]
    fn completion_notifications_are_independent_of_input_availability() {
        let mut turn = AgentTurn::default();
        assert!(!turn.report("codex", AgentEvent::Ready));
        assert!(!turn.report("codex", AgentEvent::Prompt));
        assert!(!turn.report("codex", AgentEvent::Ready));
        assert!(turn.report("codex", AgentEvent::Complete));
        assert!(!turn.report("codex", AgentEvent::Complete));
        assert!(turn.take_input("继续", true).is_ok());
        assert!(!turn.report("codex", AgentEvent::Complete));
        assert!(turn.take_input("重复", true).unwrap_err().contains("提交"));
        turn.report("codex", AgentEvent::Prompt);
        assert!(turn.report("codex", AgentEvent::Complete));
        assert!(turn.take_input("下一轮", true).is_ok());
    }

    #[test]
    fn rejects_running_blocked_and_unavailable_states_until_current_state_changes() {
        for agent in ["codex", "claude", "pi"] {
            let mut turn = AgentTurn::default();
            turn.report(agent, AgentEvent::Prompt);
            assert!(turn.take_input("继续", true).unwrap_err().contains("运行"));
            turn.report(agent, AgentEvent::Blocked);
            assert!(turn.take_input("继续", true).unwrap_err().contains("等待"));
            turn.report(agent, AgentEvent::Unblocked);
            assert!(turn.take_input("继续", true).unwrap_err().contains("运行"));
            turn.report(agent, AgentEvent::Ready);
            turn.report(agent, AgentEvent::Error);
            assert!(turn.take_input("继续", true).unwrap_err().contains("未就绪"));
            turn.report(agent, AgentEvent::Ready);
            assert!(turn.take_input("继续", true).is_ok());
            turn.report(agent, AgentEvent::End);
            assert!(turn.take_input("继续", true).is_err());
        }
    }

    #[test]
    fn an_idle_dialog_returns_to_available_without_a_new_task_or_notification() {
        let mut turn = AgentTurn::default();
        turn.report("pi", AgentEvent::Start);
        turn.report("pi", AgentEvent::Blocked);
        assert!(turn.take_input("继续", true).is_err());
        assert!(!turn.report("pi", AgentEvent::Unblocked));
        assert!(turn.take_input("继续", true).is_ok());
        turn.reject_input();
        assert!(turn.take_input("继续", true).is_err());
        assert!(!turn.report("pi", AgentEvent::Ready));
        assert!(turn.take_input("继续", true).is_ok());
    }

    #[test]
    fn codex_preserves_paragraphs_and_separates_submission_from_paste() {
        for agent in ["codex", "claude", "pi"] {
            let mut turn = AgentTurn::default();
            turn.report(agent, AgentEvent::Prompt);
            assert!(turn.report(agent, AgentEvent::Complete));
            let input = turn.take_input("第一段\n\n第三段", true).unwrap();
            let (expected, delay) = if agent == "codex" {
                ("\x1b[200~第一段\x1b[201~\x1b\r\x1b[200~\x1b[201~\x1b\r\x1b[200~第三段\x1b[201~", Duration::from_millis(300))
            } else {
                ("\x1b[200~第一段\r\r第三段\x1b[201~", Duration::ZERO)
            };
            assert_eq!(input.data, expected, "{agent}");
            assert_eq!(input.submit_delay, delay, "{agent}");
            assert!(turn.take_input("重复", true).is_err());
            turn.report(agent, AgentEvent::Ready);
            assert!(turn.take_input("多\n行", false).is_err());
            assert_eq!(turn.take_input("单行", false).unwrap().data, "单行");
        }
    }

    #[test]
    fn legacy_pi_stop_neither_notifies_nor_unlocks_a_running_task() {
        let mut turn = AgentTurn::default();
        turn.report("pi", AgentEvent::Prompt);
        assert!(!turn.report("pi", AgentEvent::Stop));
        assert!(turn.take_input("继续", true).is_err());
        assert!(turn.report("pi", AgentEvent::Complete));
        assert!(turn.take_input("继续", true).is_ok());
        turn.report("pi", AgentEvent::Prompt);
        turn.report("pi", AgentEvent::Error);
        assert!(!turn.report("pi", AgentEvent::Complete));
    }

    #[test]
    fn rejects_commands_and_non_text() {
        for value in [" /model", "!echo", "\x03", "a\tb", "a\rb", "a\x1b[A", "   "] {
            assert_eq!(plain_agent_text(value), None, "{value:?}");
        }
        assert_eq!(plain_agent_text(" 中文\r\n继续 "), Some("中文\n继续".into()));
    }
}
