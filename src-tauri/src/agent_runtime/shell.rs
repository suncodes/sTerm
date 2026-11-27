//! 当前 Shell 进程内的 Agent 包装与生命周期，不改写用户配置。
use super::{launcher, State};
use axum::{extract::{ConnectInfo, DefaultBodyLimit, Path, State as AxumState}, http::{HeaderMap, StatusCode}, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, fs, net::{Ipv4Addr, SocketAddr}, path::PathBuf, sync::{Arc, Mutex}};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct ShellState {
    service: tokio::sync::Mutex<Option<Service>>,
    bindings: Arc<Mutex<HashMap<String, Binding>>>,
    owned: Arc<Mutex<HashMap<String, NativeProcess>>>,
}
struct Service { address: String, shutdown: Arc<tokio::sync::Notify> }
impl Drop for Service { fn drop(&mut self) { self.shutdown.notify_one(); } }
#[derive(Clone)]
struct Binding { tab: u32, generation: u32, token: String }
struct NativeProcess { tab: u32, generation: u32, program: PathBuf, job: Option<crate::tunnel_process::Job> }
pub struct Bootstrap { pub script: PathBuf, pub address: String, pub token: String }
#[derive(Deserialize)]
struct LaunchRequest { agent: String, cwd: String, args: Vec<String> }
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct Started { runtime_id: String, pid: u32 }
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct Finished { runtime_id: String }

pub fn enabled(app: &AppHandle) -> bool {
    crate::app_dirs::directory(app,"data").ok().and_then(|dir| fs::read(dir.join("agent-access.json")).ok())
        .and_then(|value|serde_json::from_slice::<Value>(&value).ok()).and_then(|v|v["enabled"].as_bool()).unwrap_or(true)
}
#[tauri::command]
pub fn agent_access_status(app: AppHandle) -> Value { json!({"enabled":enabled(&app),"shells":["pwsh","powershell","cmd"]}) }
#[tauri::command]
pub fn agent_access_set_enabled(app: AppHandle, window: tauri::WebviewWindow, enabled: bool) -> Result<(),String> {
    if window.label()!="main" {return Err("仅主窗口可修改 Agent 接入设置".into())}
    let directory=crate::app_dirs::directory(&app,"data")?;
    fs::create_dir_all(&directory).map_err(|e|e.to_string())?;
    crate::storage::write(&directory.join("agent-access.json"),json!({"enabled":enabled}).to_string().as_bytes(),false)
}
fn authorize(app:&AppHandle,id:&str,headers:&HeaderMap,peer:SocketAddr)->Result<Binding,StatusCode>{
    if !peer.ip().is_loopback()||headers.contains_key("origin"){return Err(StatusCode::UNAUTHORIZED)}
    let state=app.state::<State>();
    let bindings=state.shell.bindings.lock().unwrap();
    let binding=bindings.get(id).ok_or(StatusCode::UNAUTHORIZED)?;
    let supplied=headers.get("authorization").and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).ok_or(StatusCode::UNAUTHORIZED)?;
    let expected=ring::hmac::Key::new(ring::hmac::HMAC_SHA256,binding.token.as_bytes());
    let supplied=ring::hmac::Key::new(ring::hmac::HMAC_SHA256,supplied.as_bytes());
    if ring::hmac::verify(&supplied,b"sterm-shell",ring::hmac::sign(&expected,b"sterm-shell").as_ref()).is_err(){return Err(StatusCode::UNAUTHORIZED)}
    if !app.state::<crate::AppState>().ptys.has_generation(binding.tab,binding.generation){return Err(StatusCode::CONFLICT)}
    Ok(binding.clone())
}
type Reply=Result<Json<Value>,(StatusCode,Json<Value>)>;
fn failure(error:impl ToString)->(StatusCode,Json<Value>){(StatusCode::CONFLICT,Json(json!({"message":error.to_string()})))}
fn authorized(app:&AppHandle,id:&str,headers:&HeaderMap,peer:SocketAddr)->Result<Binding,(StatusCode,Json<Value>)>{authorize(app,id,headers,peer).map_err(|status|(status,Json(json!({"message":"Shell 接入凭据已失效"}))))}

#[derive(Default,Debug)]
struct Options { resume:Option<String>,cwd:String,settings:Value,tui:Vec<String> }
fn codex_options(args:&[String],cwd:&str)->Option<Options>{
    if args.first().is_some_and(|s|["exec","review","login","logout","mcp","mcp-server","app-server","app","completion","debug","apply","fork","cloud","features","help"].contains(&s.as_str())){return None}
    let mut result=Options{cwd:cwd.into(),settings:json!({}),..Default::default()};
    let mut index=0;
    if args.first().is_some_and(|s|s=="resume"){
        if args.len()<2||args[1].starts_with('-'){return None}
        result.resume=Some(args[1].clone());index=2;
    }
    while index<args.len(){
        let value=&args[index];
        match value.as_str(){
            "-m"|"--model"|"-s"|"--sandbox"|"-a"|"--ask-for-approval"|"-C"|"--cd"=>{
                let next=args.get(index+1)?.clone();
                match value.as_str(){
                    "-m"|"--model"=>{result.settings["model"]=json!(next);result.tui.extend([value.clone(),next]);}
                    "-s"|"--sandbox"=>{result.settings["sandbox"]=json!(next);result.tui.extend([value.clone(),next]);}
                    "-a"|"--ask-for-approval"=>{result.settings["approvalPolicy"]=json!(next);result.tui.extend([value.clone(),next]);}
                    _=>{result.cwd=next;}
                }
                index+=2;
            }
            "--no-alt-screen"=>{result.tui.push(value.clone());index+=1;}
            _ if !value.starts_with('-')&&result.resume.is_none()&&index==args.len()-1&&index==0=>{
                result.tui.push(value.clone());index+=1;
            }
            _=>return None,
        }
    }
    Some(result)
}
fn interactive_claude(args:&[String])->bool{
    !args.iter().any(|a|["-p","--print","-h","--help","-v","--version","--bare","--safe-mode","--bg","--background","--remote-control"].contains(&a.as_str()))
        &&args.first().is_none_or(|a|!["auth","agents","attach","logs","stop","kill","rm","mcp","plugin","plugins","update","upgrade","install","doctor","setup-token","purge","gateway","import","respawn","ultrareview"].contains(&a.as_str()))
}
/// 按 Windows CommandLineToArgvW 的约定传递字面参数，防止 PowerShell 再解析命令。
fn quote_argument(value:&str)->String{
    let mut result=String::from("\"");let mut slashes=0;
    for c in value.chars(){
        if c=='\\'{slashes+=1;continue}
        if c=='"'{result.push_str(&"\\".repeat(slashes*2+1));result.push(c);}
        else{result.push_str(&"\\".repeat(slashes));result.push(c);}
        slashes=0;
    }
    result.push_str(&"\\".repeat(slashes*2));result.push('"');result
}
async fn launch(AxumState(app):AxumState<AppHandle>,Path(id):Path<String>,ConnectInfo(peer):ConnectInfo<SocketAddr>,headers:HeaderMap,Json(request):Json<LaunchRequest>)->Reply{
    let binding=authorized(&app,&id,&headers,peer)?;
    if !["claude","codex"].contains(&request.agent.as_str())||request.args.len()>100||request.args.iter().any(|a|a.len()>32768||a.contains('\0')){return Err(failure("无效的 Agent 参数"))}
    let program=launcher::executable(&request.agent).map_err(failure)?;
    let options=if request.agent=="codex"{codex_options(&request.args,&request.cwd)}else if interactive_claude(&request.args){Some(Options{cwd:request.cwd.clone(),tui:request.args.clone(),settings:json!({}),..Default::default()})}else{None};
    let Some(mut options)=options else{return Ok(Json(json!({"passthrough":true,"program":program,"message":"当前命令或参数使用原生 CLI，未接入网页 Agent 模式。"})))};
    if !enabled(&app){return Ok(Json(json!({"passthrough":true,"program":program})))}
    let state=app.state::<State>();
    if state.hub.list(false)["sessions"].as_array().is_some_and(|rows|rows.iter().any(|row|row["tabId"]==binding.tab&&row["generation"]==binding.generation&&row["state"]!="exited")){return Err(failure("此终端已有 Agent 会话"))}
    if !PathBuf::from(&options.cwd).is_absolute(){
        options.cwd=PathBuf::from(&request.cwd).join(&options.cwd).to_string_lossy().into_owned();
    }
    state.hub.retire(binding.tab,binding.generation);
    if let Some(remote)=app.state::<crate::remote::RemoteState>().current(){remote.unshare(binding.tab,binding.generation);}
    if !PathBuf::from(&options.cwd).is_dir(){return Err(failure("工作目录不存在"))}
    let launch=match launcher::prepare_arguments(&app,&state,binding.tab,binding.generation,&request.agent,&options.cwd,options.resume,options.settings,options.tui).await{
        Ok(launch)=>launch,
        Err(message)=>return Ok(Json(json!({"passthrough":true,"program":program,"message":format!("Agent 网页接入未启用，继续使用原生 CLI：{message}")}))),
    };
    if !app.state::<crate::AppState>().ptys.has_generation(binding.tab,binding.generation){state.hub.remove(&launch.runtime_id);return Err(failure("终端已关闭或替换"))}
    state.shell.owned.lock().unwrap().insert(launch.runtime_id.clone(),NativeProcess{tab:binding.tab,generation:binding.generation,program:PathBuf::from(&launch.program),job:None});
    let command_line=launch.args.iter().map(|arg|quote_argument(arg)).collect::<Vec<_>>().join(" ");
    Ok(Json(json!({"runtimeId":launch.runtime_id,"program":launch.program,"commandLine":command_line,"env":launch.env,"cwd":options.cwd})))
}
async fn started(AxumState(app):AxumState<AppHandle>,Path(id):Path<String>,ConnectInfo(peer):ConnectInfo<SocketAddr>,headers:HeaderMap,Json(request):Json<Started>)->Reply{
    let binding=authorized(&app,&id,&headers,peer)?;
    let state=app.state::<State>();
    let parent=app.state::<crate::AppState>().ptys.shell_pid(binding.tab,binding.generation).ok_or_else(||failure("原 Shell 已退出"))?;
    let mut owned=state.shell.owned.lock().unwrap();
    let process=owned.get_mut(&request.runtime_id).ok_or_else(||failure("Agent 启动记录失效"))?;
    if process.tab!=binding.tab||process.generation!=binding.generation||process.job.is_some(){return Err(failure("Agent 启动记录不匹配"))}
    let job=crate::tunnel_process::Job::new().map_err(failure)?;
    job.assign_native_child(request.pid,parent,&process.program).map_err(failure)?;
    process.job=Some(job);state.hub.set_end_capability(&request.runtime_id);
    Ok(Json(json!({"ok":true})))
}
async fn finish(AxumState(app):AxumState<AppHandle>,Path(id):Path<String>,ConnectInfo(peer):ConnectInfo<SocketAddr>,headers:HeaderMap,Json(request):Json<Finished>)->Reply{
    let binding=authorized(&app,&id,&headers,peer)?;let state=app.state::<State>();
    let mut owned=state.shell.owned.lock().unwrap();
    if owned.get(&request.runtime_id).is_some_and(|p|p.tab==binding.tab&&p.generation==binding.generation){
        owned.remove(&request.runtime_id);state.hub.finish(&request.runtime_id);
    }
    Ok(Json(json!({"ok":true})))
}
impl ShellState{
    pub async fn bootstrap(&self,app:&AppHandle,tab:u32,generation:u32)->Result<Option<Bootstrap>,String>{
        if !enabled(app){return Ok(None)}
        let directory=crate::app_dirs::directory(app,"data")?.join("agent-runtime").join("shell");
        fs::create_dir_all(&directory).map_err(|e|e.to_string())?;
        let script=directory.join("agent-shell.ps1");
        fs::write(&script,format!("\u{feff}{}",include_str!("../../../integrations/agent-shell.ps1"))).map_err(|e|e.to_string())?;
        let mut service=self.service.lock().await;
        if service.is_none(){
            let listener=tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST,0)).await.map_err(|e|e.to_string())?;
            let address=format!("http://127.0.0.1:{}",listener.local_addr().map_err(|e|e.to_string())?.port());
            let shutdown=Arc::new(tokio::sync::Notify::new());let done=shutdown.clone();
            let router=Router::new().route("/shell/{id}/launch",post(launch)).route("/shell/{id}/started",post(started)).route("/shell/{id}/finish",post(finish)).layer(DefaultBodyLimit::max(64*1024)).with_state(app.clone());
            tokio::spawn(async move{let _=axum::serve(listener,router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async move{done.notified().await}).await;});
            *service=Some(Service{address,shutdown});
        }
        let id=uuid::Uuid::new_v4().simple().to_string();
        let token=format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple());
        self.bindings.lock().unwrap().insert(id.clone(),Binding{tab,generation,token:token.clone()});
        Ok(Some(Bootstrap{script,address:format!("{}/shell/{id}",service.as_ref().unwrap().address),token}))
    }
    pub fn stop(&self){self.bindings.lock().unwrap().clear();self.owned.lock().unwrap().clear();}
    pub fn retire(&self,tab:u32,generation:u32){
        self.bindings.lock().unwrap().retain(|_,b|b.tab!=tab||b.generation!=generation);
        self.owned.lock().unwrap().retain(|_,p|p.tab!=tab||p.generation!=generation);
    }
    pub fn end(&self,runtime:&str)->Result<(),String>{
        let owned=self.owned.lock().unwrap();
        let process=owned.get(runtime).ok_or("此 Agent 不是由当前 Shell 接入的进程")?;
        process.job.as_ref().ok_or("Agent 尚未启动完成")?.terminate()
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn codex_flags_are_forwarded_without_losing_native_settings(){
        let args=vec!["-m".into(),"custom-model".into(),"-C".into(),"D:/work project".into(),"-s".into(),"workspace-write".into()];
        let options=codex_options(&args,"D:/old").unwrap();
        assert_eq!(options.cwd,"D:/work project");assert_eq!(options.settings["model"],"custom-model");assert_eq!(options.settings["sandbox"],"workspace-write");
        assert!(codex_options(&["exec".into(),"task".into()],"D:/work").is_none());
        assert!(codex_options(&["resume".into(),"--last".into()],"D:/work").is_none());
    }
    #[test]fn native_windows_arguments_are_literal(){
        assert_eq!(quote_argument("hello world"),"\"hello world\"");
        assert_eq!(quote_argument("a\"b"),"\"a\\\"b\"");
        assert_eq!(quote_argument("D:\\work\\"),"\"D:\\work\\\\\"");
        assert_eq!(quote_argument("$(echo x)"),"\"$(echo x)\"");
    }
    #[test]fn claude_non_interactive_commands_do_not_create_sessions(){
        for argument in ["--help","--version","--print","--bare","auth"]{assert!(!interactive_claude(&[argument.into()]));}
        assert!(interactive_claude(&["--resume".into(),"abc".into()]));
    }
}
