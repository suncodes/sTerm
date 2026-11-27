import type { AgentInteraction, AgentSession } from "./types";
import { escapeHtml as escape } from "./html";
import { changesHtml, diffHtml, markdown } from "./presentation";

function lines(label:string,value:unknown):string {
  if(value==null||value==="")return "";
  if(Array.isArray(value))return value.map(entry=>lines(label,entry)).join("");
  if(typeof value==="object")return Object.entries(value).map(([key,entry])=>lines(label+" · "+key,entry)).join("");
  return '<div class="permission-row"><span>'+escape(label)+'</span><strong>'+escape(value)+'</strong></div>';
}
export function permissionHtml(value:unknown):string {
  if(!value||typeof value!=="object")return '<p class="sheet-description">以当前原生请求列明的范围为准。</p>';
  const data=value as Record<string,unknown>;
  return '<div class="permission-scope">'+Object.entries(data).map(([key,entry])=>lines(({network:"网络访问",fileSystem:"文件访问",read:"读取",write:"写入",readableRoots:"可读目录",writableRoots:"可写目录"} as Record<string,string>)[key]??key,entry)).join("")+'</div>';
}
export function approvalHtml(item:AgentInteraction,row?:AgentSession):string {
  const context=(item.data.context??{}) as Record<string,unknown>,input=(context.input??{}) as Record<string,unknown>;
  const nativeItem=row?.items.find(entry=>(entry.agentId||"")===(item.agentId||"")&&(entry.id===context.itemId||entry.data?.id===context.itemId)),data=nativeItem?.data??{};
  const command=context.command||input.command||data.command;
  let body=item.kind==="plan"?'<div class="plan-document">'+markdown(item.detail)+'</div>':item.detail?'<p class="approval-reason">'+escape(item.detail)+'</p>':"";
  if(command){
    const text=Array.isArray(command)?command.join(" "):String(command),parts=text.trim().split(/\r?\n/),preview=parts.find(part=>part.trim())??text;
    const long=parts.length>1||preview.length>160;
    body+='<div class="tool-command approval-command"><span>'+(long?'命令预览 · '+parts.length+' 行':'命令')+'</span><pre>'+escape(preview.slice(0,160))+(preview.length>160?'…':'')+'</pre></div>';
    if(long)body+='<details class="native-detail approval-command-detail"><summary>查看完整命令</summary><pre class="detail-code">'+escape(text)+'</pre></details>';
  }
  const cwd=context.cwd||input.cwd||data.cwd;if(cwd)body+='<p class="approval-location">Working directory: <code>'+escape(cwd)+'</code></p>';
  const host=context.networkApprovalContext as Record<string,unknown>|undefined;
  if(host)body+=lines("网络主机",host.host)+lines("协议",host.protocol);
  if(input.file_path)body+=lines("文件",input.file_path);
  let files="";
  if(input.content)files+='<pre class="detail-code">'+escape(input.content)+'</pre>';
  if(input.old_string||input.new_string)files+=diffHtml('-'+String(input.old_string??'').replace(/\n/g,'\n-')+'\n+'+String(input.new_string??'').replace(/\n/g,'\n+'));
  files+=changesHtml(data.changes);
  if(files)body+='<details class="native-detail"><summary>查看文件变更详情</summary>'+files+'</details>';
  if(context.grantRoot)body+=lines("本会话写入范围",context.grantRoot);
  if(context.additionalPermissions)body+=permissionHtml(context.additionalPermissions);
  if(item.kind==="permissions")body+=permissionHtml(item.data.permissions);
  return body;
}
export function decisionDescription(value:unknown,suggestions?:unknown):string {
  if(value==="acceptForSession")return '允许范围和有效期以此原生请求为准。';
  if(value==="allowSession")return permissionHtml(suggestions);
  if(!value||typeof value!=="object")return "";
  const choice=value as Record<string,unknown>;
  const amendment=choice.acceptWithExecpolicyAmendment as Record<string,unknown>|undefined;
  if(amendment)return '<span>允许此操作，并记住命令规则；后续匹配该规则的命令可自动获准。</span><details class="native-detail"><summary>查看将记住的完整规则</summary><code>'+escape(Array.isArray(amendment.execpolicyAmendment)?amendment.execpolicyAmendment.join(" "):JSON.stringify(amendment))+'</code></details>';
  const network=choice.applyNetworkPolicyAmendment;
  return network?'<span>将应用以下网络规则：</span>'+permissionHtml(network):'<details><summary>原生选项详情</summary><pre>'+escape(JSON.stringify(value,null,2))+'</pre></details>';
}
