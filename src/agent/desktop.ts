import { invoke } from "@tauri-apps/api/core";
import type { TabManager } from "../terminal/tabmanager";
import type { RemoteController } from "../remote/controller";
import type { AgentRoster } from "./types";

/** 保留原生终端，只展示接入/分享状态；启动通过当前 Shell 完成。 */
export function setupAgentAccess(tabs:TabManager,remote:RemoteController):void{
  const strip=document.createElement("div");strip.className="agent-share-strip";strip.hidden=true;
  const text=document.createElement("span"),settings=document.createElement("button");
  settings.textContent="分享设置";settings.type="button";
  settings.onclick=()=>window.dispatchEvent(new CustomEvent("sterm-open-settings",{detail:{page:"remote"}}));
  strip.append(text,settings);document.getElementById("term-wrap")!.prepend(strip);
  let updating=false;
  async function refresh():Promise<void>{
    if(updating)return;updating=true;
    try{
      const result=await invoke<AgentRoster>("agent_runtime_list");
      await remote.updateAgents(result.sessions);
      const row=result.sessions.find(row=>row.tabId===tabs.active?.id&&row.generation===tabs.active?.remoteInfo().generation);
      strip.hidden=!row;
      if(row)text.textContent=(row.state==="exited"?"Agent 已结束 · 网页保留内容 · ":"Agent 已接入 · ")+(!row.shared?"当前未分享":row.allowControl?"网页允许操作 · 电脑可继续输入":"网页只读 · 电脑可继续输入");
    }catch(error){console.warn("同步 Agent 接入状态失败",error);}
    finally{updating=false;}
  }
  window.setInterval(()=>void refresh(),1000);void refresh();
}
