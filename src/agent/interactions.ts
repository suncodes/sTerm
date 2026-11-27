import type { AgentInteraction, AgentSession } from "./types";

export const escapeHtml = (value: unknown): string => String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
const e = escapeHtml;
type Field = { format?: string; type?: string; title?: string; description?: string; enum?: unknown[]; enumNames?: string[]; items?: Field; default?: unknown; oneOf?: { const: unknown; title?: string }[]; anyOf?: unknown; minLength?: number; maxLength?: number; minimum?: number; maximum?: number };
function normalizedSchema(schema: Record<string,unknown>):Record<string,unknown>{
  const normalized=JSON.parse(JSON.stringify(schema)) as Record<string,unknown>;
  for(const field of Object.values(normalized.properties??{}) as Field[]){
    if(Array.isArray(field.oneOf)&&field.oneOf.length&&field.oneOf.every(v=>typeof v.const==="string"&&Object.keys(v).every(k=>["const","title"].includes(k)))){
      field.enum=field.oneOf.map(v=>v.const);field.enumNames=field.oneOf.map(v=>v.title??String(v.const));delete field.oneOf;
    }
    const options=field.items?.anyOf as {const:string;title?:string}[]|undefined;
    if(Array.isArray(options)&&options.length&&options.every(v=>typeof v.const==="string"&&Object.keys(v).every(k=>["const","title"].includes(k))))field.items={type:"string",enum:options.map(v=>v.const),enumNames:options.map(v=>v.title??v.const)};
  }
  return normalized;
}
export function formSupported(schema: Record<string, unknown> | undefined): boolean {
  if(schema)schema=normalizedSchema(schema);
  if (!schema || schema.type !== "object" || !schema.properties || typeof schema.properties !== "object") return false;
  const keys = new Set(["type","title","description","properties","required","additionalProperties","$schema"]);
  if (Object.keys(schema).some(key => !keys.has(key))) return false;
  return Object.values(schema.properties).every((f: Field) => {
    if (!f || f.format&&!["email","uri","date","date-time"].includes(f.format) || Object.keys(f).some(k => !["type","title","description","enum","enumNames","default","items","minLength","maxLength","minimum","maximum","minItems","maxItems","uniqueItems","format"].includes(k))) return false;
    return ["string","number","integer","boolean"].includes(f.type ?? "") || f.type === "array" && f.items?.type === "string" && Array.isArray(f.items.enum);
  });
}
export function interactionHtml(item: AgentInteraction, enabled: boolean): string {
  const disabled = enabled ? "" : " disabled";
  const button = (label: string, attrs: string) => '<button type="button" class="subtle-button" '+attrs+disabled+'>'+e(label)+'</button>';
  let body = '<p class="sheet-description">'+e(item.agentId ? "来自子 Agent：" + item.agentId : "来自主 Agent")+' · '+(item.blocking ? "正在等待回答" : "可随时回答，Agent 可以继续")+'</p>';
  if (item.detail) body += '<pre class="detail-code">'+e(item.detail)+'</pre>';
  if (item.data.context) body += '<details><summary>原生请求详情及权限规则</summary><pre class="detail-code">'+e(JSON.stringify(item.data.context,null,2))+'</pre></details>';
  if (["question","asyncQuestion"].includes(item.kind)) {
    body += '<form id="interaction-form" data-interaction="'+e(item.id)+'">';
    for (const [index,q] of (item.data.questions ?? []).entries()) {
      body += '<fieldset><legend>'+e(q.header ? q.header + " · " : "")+e(q.title)+(q.multiSelect ? "（可多选）" : "")+'</legend>';
      if (q.preview) body += '<pre class="detail-code">'+e(q.preview)+'</pre>';
      for (const [choice,option] of (q.options ?? []).entries()) body += '<label class="question-option"><input type="'+(q.multiSelect ? "checkbox" : "radio")+'" name="q'+index+'" value="'+choice+'"'+disabled+'><span><strong>'+e(option.label)+'</strong>'+(option.description?'<small>'+e(option.description)+'</small>':"")+(option.preview?'<pre class="detail-code">'+e(option.preview)+'</pre>':"")+'</span></label>';
      body += '<label class="question-notes">'+(q.options?.length ? "其他回答 / 补充说明" : "你的回答")+(q.secret?'<input type="password" name="notes'+index+'" autocomplete="off" maxlength="8192"'+disabled+'>':'<textarea name="notes'+index+'" maxlength="8192" rows="2"'+disabled+'></textarea>')+'</label></fieldset>';
    }
    body += '<div class="confirm-actions">'+(item.data.cancellable?button("取消提问",'data-interaction-cancel'):"")+'<button type="submit" class="subtle-button"'+disabled+'>提交回答</button></div></form>';
  } else if (item.kind === "approval" || item.kind === "plan") {
    body += '<div class="interaction-decisions">'+(item.data.decisions ?? []).map((choice,index)=>button(choice.label,'data-interaction-decision="'+index+'"')+(typeof choice.value==="object"?'<pre class="decision-rule">'+e(JSON.stringify(choice.value,null,2))+'</pre>':"")).join("")+'</div>';
  } else if (item.kind === "permissions") {
    body += '<pre class="detail-code">'+e(JSON.stringify(item.data.permissions,null,2))+'</pre><div class="interaction-decisions">'+button("仅本回合允许",'data-permission-scope="turn"')+button("本会话允许",'data-permission-scope="session"')+button("拒绝",'data-permission-scope="deny"')+'</div>';
  } else if (item.kind === "form" && formSupported(item.data.schema)) {
    const schema = normalizedSchema(item.data.schema!), fields = schema.properties as Record<string,Field>, required = (schema.required ?? []) as string[];
    body += '<form id="interaction-form" data-interaction="'+e(item.id)+'">';
    for (const [index,[name,f]] of Object.entries(fields).entries()) {
      const attrs = ' name="f'+index+'"'+(required.includes(name)?" required":"")+disabled;
      body += '<label class="form-field">'+e(f.title ?? name)+(required.includes(name)?" *":"")+(f.description?'<small>'+e(f.description)+'</small>':"");
      if (f.type === "boolean") body += '<select'+attrs+'><option value="">请选择</option><option value="true">是</option><option value="false">否</option></select>';
      else if (f.enum) body += '<select'+attrs+'><option value="">请选择</option>'+f.enum.map((v,i)=>'<option value="'+i+'">'+e(f.enumNames?.[i]??v)+'</option>').join("")+'</select>';
      else if (f.type === "array") body += '<select multiple'+attrs+'>'+f.items!.enum!.map((v,i)=>'<option value="'+i+'">'+e(f.items?.enumNames?.[i]??v)+'</option>').join("")+'</select>';
      else body += '<input type="'+(f.type==="string"?(f.format==="email"?"email":f.format==="uri"?"url":f.format==="date"?"date":"text"):"number")+'"'+attrs+(f.type==="integer"?' step="1"':f.type==="number"?' step="any"':"")+(f.minLength!=null?' minlength="'+f.minLength+'"':"")+(f.maxLength!=null?' maxlength="'+f.maxLength+'"':"")+(f.minimum!=null?' min="'+f.minimum+'"':"")+(f.maximum!=null?' max="'+f.maximum+'"':"")+'>';
      body += '</label>';
    }
    body += '<div class="confirm-actions">'+button("拒绝",'data-form-action="decline"')+button("取消",'data-form-action="cancel"')+'<button type="submit" class="subtle-button"'+disabled+'>提交表单</button></div></form>';
  } else if (item.kind === "url") {
    let url: URL | undefined; try { const parsed = new URL(item.data.url ?? ""); if (["http:","https:"].includes(parsed.protocol)) url = parsed; } catch { /* 仅开放原生提供的 HTTP(S) URL。 */ }
    body += url ? '<p class="sheet-description">打开原生工具提供的授权页面，完成后返回确认。</p><a class="external-link" href="'+e(url.href)+'" target="_blank" rel="noopener noreferrer">打开 '+e(url.host)+'</a><div class="interaction-decisions">'+button("已完成",'data-form-action="accept"')+button("拒绝",'data-form-action="decline"')+button("取消",'data-form-action="cancel"')+'</div>' : '<p class="sheet-description">授权地址不可用，请在电脑端完成。</p>';
  } else {
    body += '<p class="sheet-description">此交互的原生界面暂不能远程操作，请在电脑端完成。</p>';
    if (item.kind === "form") body += '<pre class="detail-code">'+e(JSON.stringify(item.data.schema,null,2))+'</pre>';
  }
  return body;
}
export function readInteraction(item: AgentInteraction, form: HTMLFormElement): unknown {
  const data = new FormData(form);
  if (item.kind === "question" || item.kind === "asyncQuestion") {
    const answers: Record<string,string[]> = {};
    for (const [index,q] of (item.data.questions ?? []).entries()) {
      const selected = data.getAll("q"+index).map(v=>q.options?.[Number(v)]?.label).filter((v):v is string=>Boolean(v));
      const notes = String(data.get("notes"+index) ?? "").trim();
      if (notes) {
        if (q.multiSelect) selected.push(notes); else selected.splice(0,selected.length, selected.length ? selected[0]+"\n"+notes : notes);
      }
      if (!selected.length) throw new Error("请回答全部问题");
      answers[q.id] = selected;
    }
    return { answers };
  }
  const content: Record<string,unknown> = {};
  for (const [index,[key,f]] of Object.entries(normalizedSchema(item.data.schema!).properties as Record<string,Field>).entries()) {
    const raw = String(data.get("f"+index) ?? "");
    if (!raw && f.type !== "array") continue;
    content[key] = f.type === "array" ? data.getAll("f"+index).map(v=>f.items!.enum![Number(v)]) : f.enum ? f.enum[Number(raw)] : f.type==="boolean" ? raw==="true" : ["number","integer"].includes(f.type!) ? Number(raw) : raw;
  }
  return { action: "accept", content };
}
export const taskLabel=(state:string):string=>({pending:"等待启动",running:"运行中",waiting:"等待回答",idle:"空闲",completed:"已完成",failed:"失败",killed:"已结束",retrying:"重试中",interrupted:"已中断",blocked:"需要电脑端处理",unknown:"状态待确认"}[state]??state);
export function tasksHtml(row: AgentSession): string {
  return (row.tasks ?? []).map(task=>'<button class="sheet-list-button" data-task="'+e(task.id)+'"><span><strong>'+e(task.title||task.id)+'</strong><small>'+e(taskLabel(task.state))+' · '+e(task.model)+(task.parentId?' · 上级 '+e(task.parentId):"")+'</small>'+(task.message?'<small>'+e(task.message)+'</small>':"")+'</span></button>').join("");
}
