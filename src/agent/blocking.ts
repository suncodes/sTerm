import { escapeHtml as e } from "./html";
import type { AgentSession } from "./types";

const text = (value: unknown, limit: number): string => typeof value === "string" && value.trim() && new TextEncoder().encode(value).length <= limit ? value : "";

export function blockReview(row: AgentSession) {
  const details = row.activity?.details;
  const explanation = text(details?.detailedExplanation, 64 * 1024);
  const steer = text(details?.steer?.message, 1024);
  const turnId = text(row.activity?.turnId, 256), reviewId = text(row.activity?.reviewId, 256);
  const latest = [...(row.turns ?? [])].reverse().find(turn => !turn.agentId);
  return {
    explanation, steer, turnId, reviewId,
    key: JSON.stringify([row.runtimeId, row.sessionId, row.generation, turnId, reviewId, explanation, steer]),
    canContinue: row.agent === "codex" && row.state === "blocked" && Boolean(row.capabilities.send && explanation && steer && turnId && reviewId)
      && (!latest || latest.id === turnId && latest.state !== "inProgress") && row.activity?.continuationStatus !== "submitted",
  };
}

export function blockedHtml(row: AgentSession, enabled: boolean, pending: boolean): string {
  const review = blockReview(row), submitted = row.activity?.continuationStatus === "submitted", ending = row.state === "blocked" && [...(row.turns ?? [])].reverse().find(turn => !turn.agentId)?.state === "inProgress";
  return '<div class="interaction-card-heading"><div><strong>' + (review.explanation ? "会话因安全检查暂停" : "会话已被安全系统停止") + '</strong><small>Codex · ' + (submitted ? "正在等待原生状态确认" : ending ? "等待原生结束当前回合" : review.canContinue ? "查看发现后决定是否继续" : "当前没有可用的继续确认") + '</small></div></div>'
    + '<div class="interaction-card-body"><p class="block-message">' + e(row.activity?.message || "Chat stopped as a precaution") + '</p>'
    + (review.explanation ? '<details class="block-findings"><summary>查看原生说明</summary><div class="block-explanation">' + e(review.explanation) + "</div>"
      + (review.steer ? '<p class="block-instruction-label">确认继续时，Codex 将收到以下要求：</p><pre class="block-steer">' + e(review.steer) + "</pre>" : "")
      + (review.canContinue ? '<label class="block-acknowledge"><input type="checkbox" data-block-acknowledge' + (!enabled || pending ? " disabled" : "") + ">我已阅读原生说明和继续要求</label>" : "") + "</details>" : "")
    + (!review.canContinue && !submitted && !ending ? '<p class="block-guidance">原生未提供当前可用的继续材料。可以切换到其他已共享会话；新建或恢复其他原生会话需使用 CLI 的菜单。</p>' : "")
    + (ending ? '<p class="block-guidance">原生正在结束当前回合，完成后将显示可用操作。</p>' : "")
    + (row.activity?.continuationStatus === "rejected" ? '<p class="block-guidance">原生未接受上一次继续请求，请重新查看说明。</p>' : "")
    + (submitted ? '<p class="block-guidance">继续请求已经提交，等待原生确认；结果不明时不会重复发送。</p>' : "")
    + '<div class="block-actions">' + (review.canContinue || submitted ? '<button class="subtle-button" data-block-continue disabled>' + (pending || submitted ? "等待原生确认…" : "确认说明并继续") + "</button>" : "")
    + '<button class="subtle-button" data-open="sessions">切换共享会话</button></div></div>';
}
