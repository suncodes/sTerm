import { marked } from "marked";
import DOMPurify from "dompurify";
import { escapeHtml as escape } from "./html";
import { commandOutput, commandText } from "./activity";
import type { AgentItem, AgentSession } from "./types";

export const runningStates=["running","inProgress","pending","waiting","retrying","in_progress"];
export const stateLabel=(value:string):string=>({running:"运行中",inProgress:"运行中",in_progress:"运行中",pending:"等待启动",waiting:"等待回答",retrying:"重试中",paused:"已暂停",blocked:"安全阻断",completed:"已完成",succeeded:"已完成",failed:"失败",error:"失败",declined:"已拒绝",interrupted:"已中断",stopped:"已停止",killed:"已结束",unknown:"状态未确认",idle:"空闲"}[value]??"状态未确认");
export function markdown(text:string):string {
  return DOMPurify.sanitize(marked.parse(text,{async:false}) as string,{FORBID_TAGS:["img","iframe","style","form"],FORBID_ATTR:["style"]});
}
export function durationLabel(value:unknown):string {
  if(typeof value!=="number"||!Number.isFinite(value)||value<0)return "";
  const seconds=Math.floor(value/1000),minutes=Math.floor(seconds/60),hours=Math.floor(minutes/60);
  return hours?`${hours} 小时 ${minutes%60} 分 ${seconds%60} 秒`:minutes?`${minutes} 分 ${seconds%60} 秒`:`${seconds} 秒`;
}
export function recordDuration(data:Record<string,unknown>|undefined):string {
  if(!data)return "";
  const value=typeof data.durationMs==="number"?data.durationMs:typeof data.startedAtMs==="number"&&typeof data.completedAtMs==="number"?data.completedAtMs-data.startedAtMs:undefined;
  const label=durationLabel(value);return label?(data.timeSource==="observed"?"约 ":"")+label:"";
}
export function taskTimingHtml(task:{state:string;data?:Record<string,unknown>}):string{
  const duration=recordDuration(task.data);if(duration)return escape(duration);
  const start=task.data?.startedAtMs;
  return runningStates.includes(task.state)&&typeof start==="number"&&Number.isFinite(start)?'<span data-task-started="'+start+'" data-time-source="'+escape(task.data?.timeSource??"native")+'">计时中</span>':"";
}
export function diffHtml(text:unknown):string {
  if(typeof text!=="string"||!text)return "";
  return '<pre class="diff-preview">'+text.split("\n").map(line=>'<span class="'+(line.startsWith("+")&&!line.startsWith("+++")?"diff-add":line.startsWith("-")&&!line.startsWith("---")?"diff-remove":line.startsWith("@@")?"diff-location":"")+'">'+escape(line)+'</span>').join("\n")+'</pre>';
}
export function changesHtml(value:unknown):string {
  if(!Array.isArray(value))return "";
  return value.map(change=>'<section class="file-change"><strong>'+escape(change.path||change.file_path||"文件变更")+'</strong>'+diffHtml(change.diff||change.patch)+(change.kind?'<small>'+escape(typeof change.kind==="string"?change.kind:change.kind.type||"")+'</small>':"")+'</section>').join("");
}
function attachmentHtml(value:unknown):string {
  if(!Array.isArray(value))return "";
  return value.map(part=>{
    const block=part as Record<string,unknown>,type=String(block.type??""),source=block.source as {type?:string;media_type?:string;data?:string}|undefined;
    if(["text","inputText","input_text","output_text"].includes(type))return "";
    let url=String(block.image_url??block.imageUrl??(type==="image"?block.url:"")??"");
    if(source?.type==="base64")url='data:'+source.media_type+';base64,'+source.data;
    if(type==="image"&&block.data&&block.mimeType)url='data:'+block.mimeType+';base64,'+block.data;
    if(/^data:image\/(png|jpeg|gif|webp);base64,[A-Za-z0-9+/=\s]+$/.test(url))return '<img class="agent-image" src="'+escape(url)+'" alt="图片附件" loading="lazy">';
    if(["localImage","image","input_image","inputImage","audio","input_audio","inputAudio","localAudio","document","resource","resource_link"].includes(type)){
      return '<p class="resource-placeholder">'+escape(type.toLowerCase().includes("audio")?"音频附件":type.toLowerCase().includes("image")?"图片附件":"资源附件")+(block.path||block.name?" · "+escape(block.path||block.name):"")+' · 请在电脑端查看</p>';
    }
    if(type==="skill"||type==="mention")return '<p class="resource-placeholder">'+escape(type==="skill"?"技能":"引用")+': '+escape(block.name)+' · '+escape(block.path)+'</p>';
    return "";
  }).join("");
}
export function itemHtml(item:AgentItem,row:AgentSession):string {
  const data=item.data??{},input=(data.input??{}) as Record<string,unknown>,native=String(data.type??"");
  let body="";
  const command=commandText(item);
  if(command)body+='<div class="tool-command"><pre>'+escape(command)+'</pre>'+(data.cwd||input.cwd?'<small>目录：'+escape(data.cwd||input.cwd)+'</small>':"")+'</div>';
  if(data.changes)body+=changesHtml(data.changes);
  if(data.diff||data.patch)body+=diffHtml(data.diff||data.patch);
  if(input.file_path){body+='<div class="file-target">'+escape(input.file_path)+'</div>';if(input.content)body+='<pre>'+escape(input.content)+'</pre>';if(input.old_string||input.new_string)body+=diffHtml('-'+String(input.old_string??'').replace(/\n/g,'\n-')+'\n+'+String(input.new_string??'').replace(/\n/g,'\n+'));}
  if(Array.isArray(data.entries))body+='<ol class="plan-entries">'+data.entries.map(entry=>{const value=entry as Record<string,unknown>;return '<li data-state="'+escape(value.status)+'"><span>'+escape(stateLabel(String(value.status??"pending")))+'</span>'+escape(value.step||value.content||value.text)+'</li>';}).join("")+'</ol>';
  if(item.kind==="image"){
    const source=data.source as {type?:string;media_type?:string;data?:string}|undefined;
    const imageUrl=source?.type==="base64"&&/^image\/(png|jpeg|gif|webp)$/.test(source.media_type??"")&&source.data?'data:'+source.media_type+';base64,'+source.data:typeof data.path==="string"||typeof data.savedPath==="string"?'/api/agents/'+encodeURIComponent(row.runtimeId)+'/media?item='+encodeURIComponent(item.id):"";
    body+=imageUrl?'<a href="'+escape(imageUrl)+'" target="_blank" rel="noopener noreferrer"><img class="agent-image" src="'+escape(imageUrl)+'" alt="'+escape(item.title)+'" loading="lazy"></a>':'<p class="resource-placeholder">'+escape(data.failure?"图片生成失败":runningStates.includes(item.status)?"正在生成图片…":"媒体内容暂不能预览，请在电脑端查看")+'</p>';
  }
  if(native==="webSearch")body+='<div class="tool-command">'+escape(data.query||(data.action as Record<string,unknown>|undefined)?.query||item.text||"网页搜索")+'</div>';
  if(data.stdin)body+='<details><summary>向命令输入</summary><pre>'+escape(data.stdin)+'</pre></details>';
  if(data.progress)body+='<p class="tool-progress">'+escape(data.progress)+'</p>';
  body+=attachmentHtml(data.content)+attachmentHtml((data.result as Record<string,unknown>|undefined)?.content)+attachmentHtml(data.contentItems)+attachmentHtml(data.output);
  if(Array.isArray(data.questionReplies)){
    for(const reply of data.questionReplies as {question:string;answer:string;secret?:boolean}[]){
      const answer=reply.secret?"已提交秘密回答":reply.answer;
      body+='<details class="question-answer"><summary>已回答 · '+escape(answer.replace(/\s+/g," ").slice(0,96))+'</summary><blockquote>'+escape(reply.question)+'</blockquote><pre>'+escape(answer)+'</pre></details>';
    }
  }
  if(item.text&&native!=="webSearch"){
    const text=command?commandOutput(item):item.text;
    if(text)body+=item.kind==="tool"||item.kind==="user"?'<pre>'+escape(text)+'</pre>':markdown(text);
  }
  const exit=typeof data.exitCode==="number"?'退出码 '+data.exitCode:"",duration=recordDuration(data);
  if((item.kind==="tool"||item.kind==="activity")&&(exit||duration))body+='<p class="tool-metrics">'+escape([exit,duration].filter(Boolean).join(" · "))+'</p>';
  if(item.kind==="tool"&&native&&!['commandExecution','fileChange','diff','planProgress'].includes(native))body+='<details class="native-detail"><summary>技术详情</summary><pre>'+escape(JSON.stringify(data,null,2))+'</pre></details>';
  return body;
}
