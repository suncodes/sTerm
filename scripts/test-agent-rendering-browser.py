"""用受控协议快照验证真实网页；真实 CLI 链路由 test-agent-mode.ps1 验证。"""
import argparse, base64, copy, json, os, pathlib, shutil, subprocess, threading, time, urllib.request, uuid
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import websocket

parser=argparse.ArgumentParser()
parser.add_argument("chrome")
parser.add_argument("--keep-artifacts",action="store_true")
args=parser.parse_args()
workspace=pathlib.Path(__file__).resolve().parent.parent
if str(workspace).upper().startswith("C:\\"):raise RuntimeError("浏览器验证需要非 C 盘工作区")
if pathlib.Path(args.chrome).name.lower()!="chrome.exe":raise RuntimeError("仅支持 Google Chrome")
root=workspace/".tmp"/("agent-rendering-"+uuid.uuid4().hex)
root.mkdir();(root/"temp").mkdir()
now=int(time.time()*1000)
row={"protocolVersion":1,"runtimeId":"fixture","sessionId":"native","generation":1,"tabId":1,"agent":"codex","title":"Rendering verification","cwd":"D:/isolated","model":"probe","models":["probe"],"policy":"readOnly","state":"running","turnId":"turn","seq":1,"settingsRevision":1,"shared":True,"allowControl":True,"capabilities":{"send":True,"stop":True,"models":True,"policy":True,"approvals":True},"items":[],"approvals":[],"interactions":[],"tasks":[],"permissions":{},"queuedInputs":[],"activity":{},"turns":[{"id":"turn","agentId":"","state":"inProgress","startedAtMs":now-1000,"timeSource":"native"}],"notices":[],"metadata":{},"error":None}
commands=[];receipts={}
class Handler(SimpleHTTPRequestHandler):
 def __init__(self,*a,**kw):super().__init__(*a,directory=str(workspace/"dist"),**kw)
 def log_message(self,*a):pass
 def reply(self,value):
  raw=json.dumps(value).encode();self.send_response(200);self.send_header("Content-Type","application/json");self.send_header("Cache-Control","no-store");self.send_header("Content-Length",str(len(raw)));self.end_headers();self.wfile.write(raw)
 def do_GET(self):
  if self.path=="/api/auth":return self.reply({"authenticated":True})
  return super().do_GET()
 def do_POST(self):
  data=json.loads(self.rfile.read(int(self.headers.get("Content-Length","0"))) or "{}")
  if self.path=="/api/agents":return self.reply({"revision":row["seq"],"sessions":[copy.deepcopy(row)]})
  if self.path.startswith("/api/agents/fixture/commands/"):return self.reply(receipts[self.path.rsplit("/",1)[-1]])
  if self.path.startswith("/api/agents/fixture"):
   snapshot=copy.deepcopy(row);snapshot["clockMs"]=int(time.time()*1000)
   return self.reply({"snapshot":snapshot,"clockMs":snapshot["clockMs"]})
  if self.path=="/api/agents/command":
   commands.append(data);receipts[data["id"]]={"id":data["id"],"status":"pending"}
   if data.get("action")=="continueBlocked":
    row["activity"]["continuationStatus"]="submitted";row["seq"]+=1
   def ack():
    receipts[data["id"]]["status"]="accepted"
    row["interactions"]=[i for i in row["interactions"] if i["id"]!=data.get("requestId")]
    if data.get("action")=="continueBlocked":
     row.update(state="running",turnId="next-turn",activity={})
     row["turns"].append({"id":"next-turn","agentId":"","state":"inProgress","startedAtMs":int(time.time()*1000)})
    row["seq"]+=1
   threading.Timer(.7,ack).start()
   return self.reply(receipts[data["id"]])
  self.send_error(404)

class CDP:
 def __init__(self,url):self.ws=websocket.create_connection(url,suppress_origin=True,timeout=15);self.n=0
 def call(self,method,params=None):
  self.n+=1;self.ws.send(json.dumps({"id":self.n,"method":method,"params":params or {}}))
  while True:
   value=json.loads(self.ws.recv())
   if value.get("id")==self.n:
    if "error" in value:raise RuntimeError(str(value["error"]))
    return value.get("result",{})
 def evaluate(self,expression):
  value=self.call("Runtime.evaluate",{"expression":expression,"returnByValue":True,"awaitPromise":True})
  if value.get("exceptionDetails"):raise RuntimeError(str(value["exceptionDetails"]))
  return value.get("result",{}).get("value")
 def until(self,expression):
  deadline=time.monotonic()+12
  while time.monotonic()<deadline:
   if self.evaluate(expression):return
   time.sleep(.1)
  raise AssertionError(expression+" :: "+str(self.evaluate("document.body.innerText")))

def item(ident,kind,text,**data):return {"id":ident,"kind":kind,"title":ident,"text":text,"status":"running","turnId":"turn","agentId":"","data":data}
def interaction(ident,kind="question",blocking=True,**data):return {"id":ident,"kind":kind,"title":ident,"detail":"Native readable prompt","turnId":"turn","agentId":"","blocking":blocking,"data":data}
def set_interaction(value):row["interactions"]=[value];row["seq"]+=1
def set_block(details=None,review=None):
 row.update(state="blocked",turnId="",interactions=[],tasks=[],activity={"kind":"blocked","turnId":"turn","reviewId":review,"message":"This request was blocked by our safety systems. Reason: Potentially unintended activity.","details":copy.deepcopy(details)})
 row["turns"]=[{"id":"turn","agentId":"","state":"failed","durationMs":6124}];row["seq"]+=1

def tap(selector):
 browser.call("Emulation.setTouchEmulationEnabled",{"enabled":True,"maxTouchPoints":1})
 rect=browser.evaluate("(async()=>{const frame=()=>new Promise(requestAnimationFrame);await frame();await frame();const n=document.querySelector("+json.dumps(selector)+");n.scrollIntoView({block:'center'});await frame();await frame();const r=n.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()")
 browser.call("Input.dispatchTouchEvent",{"type":"touchStart","touchPoints":[{"x":rect["x"],"y":rect["y"]}]})
 time.sleep(.05)
 browser.call("Input.dispatchTouchEvent",{"type":"touchEnd","touchPoints":[]})
 browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))")

def shot(name):
 value=browser.call("Page.captureScreenshot",{"format":"png"});(root/name).write_bytes(base64.b64decode(value["data"]))

server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
chrome=None;browser=None;log=None
try:
 row["items"]=[item("before","assistant","Text before"),item("thought","thinking","Reasoning here"),item("command","tool","echo one\n\nresult",type="commandExecution",command="echo one",cwd="D:/isolated",exitCode=0,durationMs=1200),item("after","assistant","Text after")]
 profile=root/"chrome";log=(root/"chrome.log").open("wb")
 chrome=subprocess.Popen([args.chrome,"--headless=new","--remote-debugging-port=0","--user-data-dir="+str(profile),"--disk-cache-dir="+str(root/"cache"),"--crash-dumps-dir="+str(root/"crash"),"--disable-extensions","--disable-background-networking","--disable-component-update","--disable-breakpad","--no-first-run","--no-default-browser-check",f"http://127.0.0.1:{server.server_port}/agent.html"],stdout=log,stderr=log,env={**os.environ,"TEMP":str(root/"temp"),"TMP":str(root/"temp")},creationflags=subprocess.CREATE_NO_WINDOW)
 deadline=time.monotonic()+15
 while not (profile/"DevToolsActivePort").exists():
  if time.monotonic()>deadline:raise RuntimeError("Chrome 未启动")
  time.sleep(.1)
 port=int((profile/"DevToolsActivePort").read_text().splitlines()[0])
 pages=json.loads(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list").read())
 browser=CDP(next(page for page in pages if page["type"]=="page")["webSocketDebuggerUrl"])
 browser.until('document.querySelectorAll("#agent-messages>.agent-item").length===4')
 order=browser.evaluate('[...document.querySelectorAll("#agent-messages>.agent-item")].map(i=>i.dataset.item)')
 assert order==["before","thought","command","after"],order
 row["items"][1]["status"]="completed";row["state"]="idle";row["turns"][0].update(state="completed",completedAtMs=now+24000,durationMs=25000);row["seq"]+=1
 browser.until('document.querySelector("#turn-summary")?.textContent.includes("25 秒") && document.querySelector("#activity-bar").hidden')
 browser.call("Page.reload")
 browser.until('document.querySelectorAll("#agent-messages>.agent-item").length===4')
 assert browser.evaluate('[...document.querySelectorAll("#agent-messages>.agent-item")].map(i=>i.dataset.item)')==order
 print("PASS Chrome live/completed/reloaded block order and native duration",flush=True)
 browser.evaluate('document.querySelector("#agent-draft").focus()')
 set_interaction(interaction("question",questions=[{"id":"q","title":"Pick a target","other":False,"options":[{"label":"A"},{"label":"B"}]}]))
 browser.until('Boolean(document.querySelector("#interaction-card input"))')
 assert browser.evaluate('document.querySelector("#sheet-layer").hidden && document.activeElement.id==="agent-draft"')
 assert not browser.evaluate('Boolean(document.querySelector("[name=notes0]"))')
 browser.evaluate('document.querySelectorAll("[name=q0]")[1].click();document.querySelector("[data-expand-interaction]").click()')
 browser.until('!document.querySelector("#sheet-layer").hidden')
 assert browser.evaluate('document.querySelectorAll("[name=q0]")[1].checked')
 browser.evaluate('document.querySelector("[data-close-sheet]").click();document.querySelector("[data-collapse-interaction]").click();document.querySelector("#interaction-card [data-interaction-open]").click()')
 assert browser.evaluate('document.querySelectorAll("[name=q0]")[1].checked')
 browser.evaluate('document.querySelector("#interaction-form").requestSubmit()')
 browser.until('document.querySelector("#interaction-card").hidden')
 assert commands[-1]["response"]=={"answers":{"q":["B"]}}
 print("PASS Chrome ordinary card without focus theft, Other constraint, draft and native acknowledgement",flush=True)
 set_interaction(interaction("async","asyncQuestion",False,questions=[{"id":"q","title":"Optional answer","options":[]}]))
 browser.until('document.querySelector("#interaction-card").textContent.includes("async")')
 assert not browser.evaluate('Boolean(document.querySelector("#interaction-form"))')
 browser.evaluate('document.querySelector("#interaction-card [data-interaction-open]").click()')
 browser.until('Boolean(document.querySelector("[name=notes0]"))')
 browser.evaluate('const f=document.querySelector("[name=notes0]");f.value="draft preserved";f.focus();f.setSelectionRange(3,7);f.dispatchEvent(new Event("input",{bubbles:true}))')
 row["interactions"][0]["detail"]="Updated native prompt";row["seq"]+=1
 browser.until('document.querySelector("#interaction-card").textContent.includes("Updated native prompt")')
 assert browser.evaluate('document.activeElement.name==="notes0" && document.activeElement.value==="draft preserved" && document.activeElement.selectionStart===3 && document.activeElement.selectionEnd===7')
 print("PASS Chrome nonblocking card expansion and focus/selection preservation across updates",flush=True)
 set_interaction(interaction("approval","approval",decisions=[{"label":"Approve once","value":"accept"}],context={"command":"echo <safe>","cwd":"D:/isolated","reason":"Native readable prompt"}))
 browser.until('Boolean(document.querySelector(".tool-command")) && document.querySelector("#interaction-card").textContent.includes("echo <safe>")')
 assert browser.evaluate('document.querySelector("#sheet-layer").hidden && !document.querySelector(".native-detail").open')
 row["tasks"]=[{"id":"shell","parentId":"","kind":"shell","title":"Background command","model":"","state":"running","turnId":"turn","message":"independent output","data":{"backgroundConfirmed":True,"startedAtMs":now}}];row["seq"]+=1
 browser.until('document.querySelector("#activity-text").textContent.includes("1 项任务仍在运行")')
 browser.call("Emulation.setDeviceMetricsOverride",{"width":390,"height":844,"deviceScaleFactor":1,"mobile":True})
 time.sleep(.4);shot("approval-mobile.png")
 assert browser.evaluate('document.querySelector("#conversation").clientHeight>=60 && document.documentElement.scrollWidth<=390')
 browser.evaluate('document.querySelector("[data-expand-interaction]").click()');shot("approval-detail.png")
 browser.evaluate('document.querySelector("[data-close-sheet]").click()')
 set_interaction(interaction("form","form",schema={"type":"object","properties":{"name":{"type":"string","default":"hello"},"choice":{"type":"boolean","default":False},"features":{"type":"array","items":{"type":"string","enum":["A","B"]},"default":["B"]}}}))
 browser.until('document.querySelector("[name=f0]")?.value==="hello"')
 assert browser.evaluate('document.querySelector("[name=f1]").value==="false" && document.querySelectorAll("[name=f2] option")[1].selected')
 browser.evaluate('document.querySelector("#interaction-form").requestSubmit()')
 browser.until('document.querySelector("#interaction-card").hidden')
 assert commands[-1]["response"]["content"]=={"name":"hello","choice":False,"features":["B"]}
 print("PASS Chrome readable approval, mobile layout, independent background work and form defaults",flush=True)
 row["tasks"]=[
  {"id":"foreground","kind":"shell","state":"running","title":"Foreground command","data":{"backgroundConfirmed":False,"itemId":"command"}},
  {"id":"failed","kind":"agent","state":"failed","title":"Past failure"},
  {"id":"done","kind":"shell","state":"completed","title":"Completed background","data":{"backgroundConfirmed":True}},
 ];row["seq"]+=1
 browser.until('document.querySelector("#task-bar").hidden && !document.querySelector("#activity-text").textContent.includes("请查看")')
 browser.evaluate('document.querySelector("[data-open=more]").click()')
 browser.until('Boolean(document.querySelector("#sheet [data-open=tasks]"))')
 browser.evaluate('document.querySelector("#sheet [data-open=tasks]").click()')
 browser.until('document.querySelector("#sheet-title").textContent==="任务记录"')
 assert browser.evaluate('document.querySelector("#sheet").textContent.includes("Past failure")')
 browser.evaluate('document.querySelector("[data-close-sheet]").click()')
 row["tasks"].append({"id":"child-live","kind":"agent","state":"running","title":"Independent child"});row["seq"]+=1
 browser.until('!document.querySelector("#task-bar").hidden && document.querySelector("#task-text").textContent.includes("1 项任务进行中")')
 row["tasks"][-1]["state"]="completed";row["seq"]+=1
 browser.until('document.querySelector("#task-bar").hidden')
 print("PASS Chrome completed/failed/foreground tasks leave activity while history and independent children remain available",flush=True)

 row["notices"]=[{"id":"info","title":"Native info","message":"Completed ordinary notice","severity":"info"}];row["seq"]+=1
 browser.until('!document.querySelector("#toast").hidden && document.querySelector("#toast").textContent==="Completed ordinary notice"')
 assert browser.evaluate('document.querySelector("#native-notice").hidden')
 browser.call("Page.reload")
 browser.until('document.querySelectorAll("#agent-messages>.agent-item").length===4')
 assert browser.evaluate('document.querySelector("#toast").hidden && document.querySelector("#native-notice").hidden')
 row["notices"].append({"id":"warn","title":"Warning","message":"Please review the runtime warning","severity":"warning"});row["seq"]+=1
 browser.until('!document.querySelector("#native-notice").hidden')
 tap("#dismiss-native-notice")
 browser.until('document.querySelector("#native-notice").hidden')
 row["seq"]+=1;time.sleep(.5)
 assert browser.evaluate('document.querySelector("#native-notice").hidden')
 row["notices"][-1]["message"]="Updated runtime warning";row["seq"]+=1
 browser.until('!document.querySelector("#native-notice").hidden && document.querySelector("#native-notice-text").textContent==="Updated runtime warning"')
 browser.until('document.querySelector("#native-notice").hidden')
 row.update(state="running",turnId="turn")
 row["notices"].append({"id":"hook","turnId":"turn","title":"Hook","message":"Hook is running","severity":"info","phase":"running"});row["seq"]+=1
 browser.until('!document.querySelector("#native-notice").hidden && document.querySelector("#native-notice-text").textContent==="Hook is running"')
 row["notices"][-1].update(phase="completed",message="Hook completed");row["seq"]+=1
 browser.until('document.querySelector("#native-notice").hidden')
 row["notices"][-1].update(phase="running",message="Another hook");row["seq"]+=1
 browser.until('!document.querySelector("#native-notice").hidden')
 row.update(state="retrying",activity={"kind":"retrying","message":"API retry in progress"});row["seq"]+=1
 browser.until('!document.querySelector("#agent-activity-detail").hidden')
 row["state"]="idle";row["seq"]+=1
 browser.until('document.querySelector("#native-notice").hidden && document.querySelector("#agent-activity-detail").hidden')
 row["error"]="Historical runtime error";row["seq"]+=1
 browser.until('!document.querySelector("#agent-notice").hidden')
 tap("#dismiss-notice");row["seq"]+=1;time.sleep(.5)
 assert browser.evaluate('document.querySelector("#agent-notice").hidden')
 row["error"]=None;row["activity"]={};row["seq"]+=1
 print("PASS Chrome notices do not replay on refresh, warnings dismiss/expire, progress/retry clears and dismissed errors stay closed",flush=True)

 def async_reply(call,index,title,answer):
  return {"questionItemId":json.dumps(["request_user_input_async",call,index],separators=(",",":")),"question":title,"answer":answer}
 def reply_text(replies):return "<send_user_message_question_reply>\n"+json.dumps(replies)+"\n</send_user_message_question_reply>"
 row["items"].extend([
  item("call-target","assistant","Choose a workspace",id="call-target",questions=[{"title":"Which workspace?"}]),
  item("answer-target","user",reply_text([async_reply("call-target",0,"Which workspace?","D:")])),
  item("answer-orphan","user",reply_text([async_reply("missing-call",0,"Choose target","B <safe>")])),
  item("call-secret","assistant","Enter a secret",id="call-secret",questions=[{"title":"Secret value","secret":True}]),
  item("answer-secret","user",reply_text([async_reply("call-secret",0,"Secret value","must-never-render")])),
 ]);row["seq"]+=1
 browser.until('Boolean(document.querySelector("[data-item=call-target] .question-answer"))')
 assert browser.evaluate('!document.querySelector("[data-item=answer-target]") && !document.querySelector("[data-item=answer-secret]") && !document.querySelector("#agent-messages").textContent.includes("send_user_message_question_reply") && !document.querySelector("#agent-messages").textContent.includes("must-never-render")')
 assert browser.evaluate('document.querySelector("[data-item=call-target] .question-answer summary").textContent.includes("D:") && document.querySelector("[data-item=answer-orphan]").textContent.includes("B <safe>") && !document.querySelector("[data-item=answer-orphan] safe")')
 browser.call("Page.reload")
 browser.until('Boolean(document.querySelector("[data-item=call-secret] .question-answer"))')
 assert browser.evaluate('!document.querySelector("[data-item=answer-target]") && !document.querySelector("#agent-messages").textContent.includes("must-never-render")')
 print("PASS Chrome native async answers fold into questions, missing questions have readable fallback, secrets redact and refresh agrees",flush=True)

 long_command="Write-Output 'preview'\n"+"\n".join("Write-Output 'script line "+str(i)+"'" for i in range(180))
 set_interaction(interaction("long-approval","approval",decisions=[
  {"label":"Approve once","value":"accept"},
  {"label":"Remember rule","value":{"acceptWithExecpolicyAmendment":{"execpolicyAmendment":["powershell","-Command",long_command]}}},
  {"label":"Cancel","value":"cancel"},
 ],context={"command":long_command,"cwd":"D:/isolated","input":{"file_path":"D:/isolated/file.txt","content":"file detail\n"*100}}))
 browser.until('Boolean(document.querySelector("#interaction-card .approval-command-detail"))')
 assert browser.evaluate('!document.querySelector(".approval-command-detail").open && document.querySelector(".interaction-scroll").textContent.includes("后续匹配该规则的命令可自动获准")')
 for width,height in [(1280,900),(760,800),(600,800),(520,800),(390,844),(360,640),(390,450)]:
  browser.call("Emulation.setDeviceMetricsOverride",{"width":width,"height":height,"deviceScaleFactor":1,"mobile":width<760})
  browser.until(f"innerWidth==={width} && innerHeight==={height}")
  browser.evaluate('(async()=>{await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);})()')
  geometry=browser.evaluate('(()=>{const rect=id=>{const r=document.getElementById(id).getBoundingClientRect();return {x:r.x,w:r.width,y:r.y,b:r.bottom}};return {messages:rect("agent-messages"),card:rect("interaction-card"),composer:rect("agent-composer"),body:rect("conversation"),buttons:[...document.querySelectorAll(".interaction-footer button")].map(b=>{const r=b.getBoundingClientRect();return {top:r.top,bottom:r.bottom,height:r.height,visible:document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)===b}}),overflow:document.documentElement.scrollWidth};})()')
  assert abs(geometry["messages"]["x"]-geometry["card"]["x"])<1.1 and abs(geometry["composer"]["x"]-geometry["card"]["x"])<1.1,(width,geometry)
  assert abs(geometry["messages"]["w"]-geometry["card"]["w"])<1.1 and abs(geometry["composer"]["w"]-geometry["card"]["w"])<1.1,(width,geometry)
  assert geometry["overflow"]<=width and geometry["body"]["b"]-geometry["body"]["y"]>20,(width,geometry)
  assert all(button["visible"] and button["height"]>=44 and button["bottom"]<=geometry["card"]["b"] for button in geometry["buttons"]),(width,geometry)
 assert browser.evaluate('(()=>{const body=document.querySelector(".interaction-scroll").getBoundingClientRect(),reason=document.querySelector(".approval-reason").getBoundingClientRect(),preview=document.querySelector(".approval-command pre").getBoundingClientRect();return reason.top>=body.top && preview.bottom<=body.bottom;})()')
 shot("compact-approval-mobile.png")
 tap(".approval-command-detail>summary")
 browser.until('document.querySelector(".approval-command-detail").open')
 browser.evaluate('document.querySelector(".interaction-scroll").scrollTop=400')
 saved_scroll=browser.evaluate('document.querySelector(".interaction-scroll").scrollTop')
 row["interactions"][0]["detail"]="Updated native approval reason";row["seq"]+=1
 browser.until('document.querySelector("#interaction-card .approval-reason").textContent==="Updated native approval reason"')
 assert browser.evaluate('document.querySelector(".approval-command-detail").open')
 assert abs(browser.evaluate('document.querySelector(".interaction-scroll").scrollTop')-saved_scroll)<1
 assert browser.evaluate('[...document.querySelectorAll(".interaction-footer button")].every(b=>{const r=b.getBoundingClientRect();return document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)===b})')
 tap('[data-interaction-decision="0"]')
 browser.until('document.querySelector("#interaction-card").hidden')
 assert commands[-1]["response"]=={"decision":"accept"}
 print("PASS Chrome shared content column at all breakpoints, long command/rule folding and visible mobile approval actions",flush=True)
 set_interaction(interaction("all-decisions","approval",decisions=[
  {"label":"批准一次","value":"accept"},{"label":"本会话允许","value":"acceptForSession"},
  {"label":"允许并记住命令规则","value":{"acceptWithExecpolicyAmendment":{"execpolicyAmendment":["powershell","-Command",long_command]}}},
  {"label":"拒绝此操作","value":"decline"},{"label":"取消本次请求","value":"cancel"},
 ],context={"command":long_command}))
 browser.until('document.querySelectorAll("#interaction-card .interaction-footer button").length===5')
 assert browser.evaluate('(()=>{const card=document.querySelector("#interaction-card").getBoundingClientRect(),footer=document.querySelector(".interaction-footer").getBoundingClientRect();return footer.bottom<=card.bottom && document.querySelector("#conversation").clientHeight>20;})()')
 tap('[data-interaction-decision="4"]')
 browser.until('document.querySelector("#interaction-card").hidden')
 assert commands[-1]["response"]=={"decision":"cancel"}
 print("PASS Chrome all five native decisions remain reachable in a short viewport",flush=True)
 set_block()
 row["interactions"]=[interaction("old-async","asyncQuestion",False,questions=[{"id":"q","title":"Stale question","options":[]}])];row["seq"]+=1
 browser.until('!document.querySelector("#blocked-card").hidden && document.querySelector("#blocked-card").textContent.includes("没有可用的继续确认")')
 assert browser.evaluate('document.querySelector("#interaction-card").hidden')
 assert browser.evaluate('document.querySelector("#composer").hidden && !document.querySelector("[data-block-continue]") && document.querySelector("#model-button").disabled && document.querySelector("#policy-button").disabled')
 assert browser.evaluate('!document.body.innerText.includes("等待电脑端处理")')
 tap('#blocked-card [data-open="sessions"]')
 browser.until('!document.querySelector("#sheet-layer").hidden && document.querySelector("#sheet-title").textContent==="会话"')
 browser.evaluate('document.querySelector("[data-close-sheet]").click()')
 set_block({"detailedExplanation":"Explanation only","reviewTarget":"opaque"},"review-incomplete")
 browser.until('Boolean(document.querySelector(".block-findings")) && !document.querySelector("[data-block-continue]")')
 print("PASS Chrome real safety error, terminal input disabled, no invented continuation and session switch",flush=True)
 native={"detailedExplanation":"Detected action "+("<unsafe>"+("x"*160)+"</unsafe>\n")*100,"steer":{"message":"Continue only within the requested scope.\nDo not delete user files."},"reviewTarget":"opaque"}
 set_block(native,"review-1")
 browser.until('Boolean(document.querySelector("[data-block-acknowledge]"))')
 assert browser.evaluate('document.querySelector("[data-block-continue]").disabled && !document.querySelector(".block-findings").open && !document.querySelector(".block-explanation unsafe")')
 for width,height in [(360,640),(390,450),(390,844)]:
  browser.call("Emulation.setDeviceMetricsOverride",{"width":width,"height":height,"deviceScaleFactor":1,"mobile":True})
  browser.until(f"innerWidth==={width} && innerHeight==={height}")
  tap(".block-findings>summary")
  browser.until('document.querySelector(".block-findings").open')
  assert browser.evaluate(f'document.documentElement.scrollWidth<={width} && document.querySelector("#blocked-card").getBoundingClientRect().bottom<={height} && document.querySelector("#conversation").clientHeight>20')
  assert browser.evaluate('document.querySelector(".block-actions button").getBoundingClientRect().height>=44')
  tap(".block-findings>summary")
  browser.until('!document.querySelector(".block-findings").open')
 tap(".block-findings>summary");tap(".block-acknowledge")
 browser.until('!document.querySelector("[data-block-continue]").disabled')
 row["seq"]+=1
 time.sleep(.45)
 assert browser.evaluate('document.querySelector("[data-block-acknowledge]").checked')
 set_block({**native,"steer":{"message":"Updated native continuation instruction."}},"review-2")
 browser.until('document.querySelector(".block-steer").textContent==="Updated native continuation instruction."')
 assert browser.evaluate('!document.querySelector("[data-block-acknowledge]").checked && !document.querySelector(".block-findings").open && document.querySelector("[data-block-continue]").disabled')
 tap(".block-findings>summary");tap(".block-acknowledge")
 browser.call("Page.reload")
 browser.until('Boolean(document.querySelector("[data-block-acknowledge]"))')
 assert browser.evaluate('!document.querySelector("[data-block-acknowledge]").checked && document.querySelector("[data-block-continue]").disabled')
 row["allowControl"]=False;row["seq"]+=1
 browser.until('document.querySelector("[data-block-acknowledge]").disabled')
 row["allowControl"]=True;row["seq"]+=1
 browser.until('!document.querySelector("[data-block-acknowledge]").disabled')
 tap(".block-findings>summary");tap(".block-acknowledge")
 browser.until('!document.querySelector("[data-block-continue]").disabled')
 shot("safety-review-mobile.png")
 count=len(commands)
 browser.evaluate('const b=document.querySelector("[data-block-continue]");b.click();b.click()')
 browser.until('document.querySelector("#blocked-card").hidden && !document.querySelector("#composer").hidden')
 assert len(commands)==count+1,commands[count:]
 assert commands[-1]["action"]=="continueBlocked" and commands[-1]["turnId"]=="turn" and commands[-1]["reviewId"]=="review-2"
 assert "text" not in commands[-1] and "message" not in commands[-1]
 print("PASS Chrome mobile touch review, long material, short viewport, readonly, refresh, stale confirmation and one native continuation",flush=True)
except Exception:
 if browser:
  shot("failure.png")
  print("Failure layout: "+str(browser.evaluate('(()=>{const n=document.querySelector(".block-findings>summary"),r=n?.getBoundingClientRect();return {viewport:[innerWidth,innerHeight,visualViewport?.width,visualViewport?.height],target:r?{x:r.x,y:r.y,w:r.width,h:r.height}:null,hit:r?document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.outerHTML:null};})()')),flush=True)
 raise
finally:
 if browser:
  try:browser.call("Browser.close")
  except Exception:pass
  browser.ws.close()
 if chrome:
  try:chrome.wait(8)
  except subprocess.TimeoutExpired:chrome.kill();chrome.wait()
 if log:log.close()
 server.shutdown();server.server_close()
 if args.keep_artifacts:print("Artifacts: "+str(root),flush=True)
 else:
  assert root.parent==(workspace/".tmp") and root.name.startswith("agent-rendering-")
  for attempt in range(5):
   try:shutil.rmtree(root);break
   except PermissionError:
    if attempt==4:raise
    time.sleep(.5)
  assert not root.exists(),str(root)
