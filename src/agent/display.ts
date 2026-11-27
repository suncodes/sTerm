import type { AgentItem, AgentSession, AgentTask } from "./types";
import { runningStates } from "./presentation";

export interface QuestionReply { id: string; call: string; index: number; question: string; answer: string; secret?: boolean }
export function parseQuestionReplies(text: string): QuestionReply[] | undefined {
  const envelope = text.trim().match(/^<send_user_message_question_reply>([\s\S]*)<\/send_user_message_question_reply>$/);
  if (!envelope) return;
  try {
    const value: unknown = JSON.parse(envelope[1]), entries = Array.isArray(value) ? value : [value];
    if (!entries.length) return;
    const replies: QuestionReply[] = [];
    for (const entry of entries) {
      if (!entry || typeof entry !== "object" || typeof entry.questionItemId !== "string" || typeof entry.question !== "string" || typeof entry.answer !== "string") return;
      const target: unknown = JSON.parse(entry.questionItemId);
      if (!Array.isArray(target) || target.length !== 3 || target[0] !== "request_user_input_async" || typeof target[1] !== "string" || !target[1] || !Number.isSafeInteger(target[2]) || target[2] < 0) return;
      replies.push({ id: entry.questionItemId, call: target[1], index: target[2], question: entry.question, answer: entry.answer });
    }
    return replies;
  } catch { return; }
}

// 只投影显示数据；原生消息、回答协议和传输身份保持原样。
export function conversationItems(row: AgentSession): AgentItem[] {
  if (row.agent !== "codex") return row.items;
  const projected = row.items.map(item => ({ ...item, data: { ...item.data } }));
  const hidden = new Set<AgentItem>();
  for (const [position, item] of projected.entries()) {
    if (item.kind !== "user") continue;
    const replies = parseQuestionReplies(item.text);
    if (!replies) continue;
    const fallback: QuestionReply[] = [];
    for (const reply of replies) {
      const source = projected.slice(0, position).find(candidate => candidate.kind === "assistant" && (candidate.agentId || "") === (item.agentId || "") && (candidate.id === reply.call || candidate.data?.id === reply.call) && Array.isArray(candidate.data?.questions) && candidate.data.questions[reply.index]);
      const question = source ? (source.data!.questions as Record<string, unknown>[])[reply.index] : undefined;
      const pending = row.interactions?.filter(entry => (entry.agentId || "") === (item.agentId || "")).flatMap(entry => entry.data.questions ?? []).find(entry => entry.id === reply.id);
      const secret = question?.secret === true || question?.isSecret === true || pending?.secret === true;
      const answer = { ...reply, question: String(question?.title || question?.question || reply.question), answer: secret ? "已提交秘密回答" : reply.answer, secret };
      if (source) {
        const answers = (source.data!.questionReplies ?? []) as QuestionReply[];
        source.data!.questionReplies = [...answers.filter(previous => previous.id !== reply.id), answer];
      } else fallback.push(answer);
    }
    const attachments = Array.isArray(item.data?.content) && item.data.content.some(part => !["text", "inputText", "input_text"].includes(String(part?.type)));
    if (!fallback.length && !attachments) hidden.add(item);
    else { item.text = ""; item.data!.questionReplies = fallback; }
  }
  return projected.filter(item => !hidden.has(item));
}

export function liveTasks(row: AgentSession): AgentTask[] {
  return (row.tasks ?? []).filter(task =>
    (task.kind !== "shell" || task.data?.backgroundConfirmed === true) &&
    (runningStates.includes(task.state) || ["unknown", "blocked", "paused"].includes(task.state))
  );
}
