import base64,os,sys,json,pathlib,time,subprocess,urllib.request,uuid,websocket,traceback
sys.stdout.reconfigure(encoding="utf-8")
ROOT=pathlib.Path(sys.argv[1]).resolve()
DRIVER=pathlib.Path(__file__).resolve().with_name("agent-mode-drive.py")
CHROME_EXE=sys.argv[2]
def drive(data):
 value=subprocess.run([sys.executable,"-X","utf8",str(DRIVER),str(ROOT),json.dumps(data)],capture_output=True,text=True,encoding="utf-8",timeout=50)
 if value.returncode:raise RuntimeError(value.stdout+value.stderr)
 return json.loads(value.stdout)["result"]
class CDP:
 def __init__(self,url):self.ws=websocket.create_connection(url,suppress_origin=True,timeout=20);self.n=0
 def call(self,method,params=None):
  self.n+=1;ident=self.n;self.ws.send(json.dumps({"id":ident,"method":method,"params":params or {}}))
  while True:
   value=json.loads(self.ws.recv())
   if value.get("id")==ident:
    if "error" in value:raise RuntimeError(str(value["error"]))
    return value.get("result",{})
 def evaluate(self,expression):
  v=self.call("Runtime.evaluate",{"expression":expression,"awaitPromise":True,"returnByValue":True,"userGesture":True})
  if v.get("exceptionDetails"):raise RuntimeError(str(v["exceptionDetails"]))
  return v.get("result",{}).get("value")
 def until(self,expression,timeout=18):
  deadline=time.monotonic()+timeout
  while time.monotonic()<deadline:
   v=self.evaluate(expression)
   if v:return v
   time.sleep(.15)
  raise RuntimeError("Timeout "+expression+" :: "+str(self.evaluate('document.body.innerText')))
 def close(self):self.ws.close()
def snapshot(runtime):return drive({"action":"snapshot","runtimeId":runtime})
def prompt(text):
 browser.evaluate('document.querySelector("#agent-draft").value='+json.dumps(text)+';document.querySelector("#agent-draft").dispatchEvent(new Event("input"));document.querySelector("#agent-composer").requestSubmit();true')
def native(tab,text):
 drive({"action":"write","tab":tab,"data":text});time.sleep(.3);drive({"action":"write","tab":tab,"data":"\r"})
def settled(runtime,timeout=15):
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  row=snapshot(runtime)
  if row["state"]=="idle":return row
  time.sleep(.2)
 raise RuntimeError("not settled "+json.dumps(row))
CHROME=None;browser=None;log=None
try:
 rows=drive({"action":"list"})["sessions"]
 claude=next(r for r in rows if r["tabId"]==900)
 codex=next(r for r in rows if r["tabId"]==901)
 for row in rows:drive({"action":"share","runtimeId":row["runtimeId"],"shared":True,"control":True})
 drive({"action":"remote_stop"})
 endpoint=drive({"action":"remote"})["origins"][0]
 for row in rows:
  deadline=time.monotonic()+25
  while not snapshot(row["runtimeId"])["sessionId"] and time.monotonic()<deadline:time.sleep(.2)
  assert snapshot(row["runtimeId"])["sessionId"],"CLI 尚未通过首次启动/信任流程"
 native(900,"LOCAL_ONE")
 settled(claude["runtimeId"])
 pair=drive({"action":"pair","origin":endpoint})["url"].replace("/#pair=","/agent.html#pair=")
 profile=ROOT/("chrome-"+uuid.uuid4().hex[:8]);profile.mkdir()
 log=(ROOT/"chrome.log").open("wb")
 env={**os.environ,"TEMP":str(ROOT/"temp"),"TMP":str(ROOT/"temp")}
 CHROME=subprocess.Popen([CHROME_EXE,"--headless=new","--disable-extensions","--remote-debugging-port=0","--user-data-dir="+str(profile),"--disk-cache-dir="+str(ROOT/"chrome-cache"),"--crash-dumps-dir="+str(ROOT/"chrome-crash"),"--disable-background-networking","--disable-component-update","--disable-breakpad","--no-first-run","--no-default-browser-check",pair],env=env,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
 for _ in range(100):
  active=profile/"DevToolsActivePort"
  if active.exists():break
  time.sleep(.1)
 port=int(active.read_text(encoding="utf8").splitlines()[0]);targets=json.loads(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list").read())
 page=next(p for p in targets if p["type"]=="page");browser=CDP(page["webSocketDebuggerUrl"])
 browser.call("Runtime.enable")
 browser.call("Page.addScriptToEvaluateOnNewDocument",{"source":"Object.defineProperty(crypto, 'randomUUID', {value:undefined});"})
 browser.evaluate("Object.defineProperty(crypto, 'randomUUID', {value:undefined});true")
 browser.until('document.querySelectorAll(".agent-session").length===4')
 browser.evaluate('document.querySelector(\'[data-runtime="'+claude["runtimeId"]+'"]\').click();true')
 browser.until('document.querySelector("#agent-messages").textContent.includes("LOCAL_ONE")')
 browser.until('!document.querySelector("#agent-draft").disabled')
 print("PASS Chrome real sTerm Agent page with four shared agents",flush=True)
 prompt("WEB_ONE")
 browser.until('document.querySelector("#agent-draft").value===""')
 row=settled(claude["runtimeId"])
 assert any(x["text"]=="WEB_ONE" for x in row["items"]),row
 print("PASS Claude browser prompt actual native history",flush=True)
 browser.evaluate('document.querySelector("#agent-model").value="haiku";document.querySelector("#agent-model").dispatchEvent(new Event("change"));true')
 browser.until('document.querySelector("#agent-model").value.includes("haiku")')
 time.sleep(.6)
 assert "haiku" in snapshot(claude["runtimeId"])["model"]
 print("PASS Claude model changes",flush=True)
 prompt("APPROVAL_WEB")
 browser.until('Boolean(document.querySelector(".agent-approval"))')
 browser.evaluate('document.querySelector(\'button[data-decision="true"]\').click();true')
 browser.until('!document.querySelector(".agent-approval")')
 row=settled(claude["runtimeId"]);assert (ROOT/"workspace"/"APPROVAL_WEB.txt").exists()
 print("PASS Claude web approval executes real write",flush=True)
 prompt("APPROVAL_NATIVE")
 browser.until('Boolean(document.querySelector(".agent-approval"))')
 time.sleep(1);drive({"action":"write","tab":900,"data":"\r"})
 browser.until('!document.querySelector(".agent-approval")')
 settled(claude["runtimeId"])
 print("PASS Claude native approval closes webpage",flush=True)
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/reset").read()
 prompt("LONG_WEB")
 browser.until('!document.querySelector("#agent-stop").disabled')
 browser.until('document.querySelector("#agent-state").textContent==="运行中"')
 browser.until('document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CLAUDE_A") && !document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CLAUDE_B")')
 time.sleep(.7)
 browser.evaluate('document.querySelector("#agent-stop").click();true')
 browser.until('document.querySelector("#agent-state").textContent==="空闲"')
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/release").read()
 print("PASS Claude browser stop",flush=True)
 browser.evaluate('document.querySelector(\'[data-runtime="'+codex["runtimeId"]+'"]\').click();true')
 browser.until('document.querySelector("#agent-title").textContent.startsWith("codex")')
 prompt("WEB_CODEX")
 browser.until('document.querySelector("#agent-messages").textContent.includes("STERM_CODEX_B")')
 browser.until('document.querySelector("#agent-draft").value===""')
 print("PASS Codex browser prompt and stream",flush=True)
 native(901,"LOCAL_CODEX")
 browser.until('document.querySelector("#agent-messages").textContent.includes("LOCAL_CODEX")')
 settled(codex["runtimeId"])
 print("PASS Codex native input streamed to real webpage",flush=True)
 prompt("APPROVAL_WEB_CODEX")
 browser.until('Boolean(document.querySelector(".agent-approval"))')
 browser.evaluate('document.querySelector(\'button[data-decision="false"]\').click();true')
 browser.until('!document.querySelector(".agent-approval")')
 settled(codex["runtimeId"])
 print("PASS Codex browser denial closes native request",flush=True)
 prompt("APPROVAL_NATIVE_CODEX")
 browser.until('Boolean(document.querySelector(".agent-approval"))')
 time.sleep(2);drive({"action":"write","tab":901,"data":"y"})
 browser.until('!document.querySelector(".agent-approval")')
 settled(codex["runtimeId"])
 print("PASS Codex native approval closes webpage",flush=True)
 browser.evaluate("location.reload();true");time.sleep(.5)
 browser.until('document.querySelectorAll(".agent-session").length===4')
 print("PASS page reload recovery",flush=True)
 # 验证四个会话的独立路由。
 for row in sorted(rows,key=lambda r:r["tabId"]):
  browser.evaluate("document.querySelector('[data-runtime="+json.dumps(row["runtimeId"])+"]').click();true")
  browser.until('!document.querySelector("#agent-draft").disabled')
  text="WEB_ROUTE_"+str(row["tabId"])
  prompt(text)
  browser.until('document.querySelector("#agent-draft").value===""')
  settled(row["runtimeId"])
  assert any(text in item["text"] for item in snapshot(row["runtimeId"])["items"]),row
 print("PASS two Claude and two Codex session routing",flush=True)
 browser.evaluate("document.querySelector('[data-runtime="+json.dumps(codex["runtimeId"])+"]').click();true")
 browser.until('!document.querySelector("#agent-model").disabled')
 browser.evaluate('document.querySelector("#agent-model").value="gpt-6.2-sol";document.querySelector("#agent-model").dispatchEvent(new Event("change"));true')
 deadline=time.monotonic()+10
 while snapshot(codex["runtimeId"])["model"]!="gpt-6.2-sol" and time.monotonic()<deadline:time.sleep(.2)
 assert snapshot(codex["runtimeId"])["model"]=="gpt-6.2-sol", browser.evaluate("document.body.innerText")
 prompt("WEB_MODEL")
 browser.until('document.querySelector("#agent-draft").value===""');settled(codex["runtimeId"])
 requests=json.loads((ROOT/"mock-requests.json").read_text(encoding="utf8"))
 assert any(r.get("model")=="gpt-6.2-sol" for r in requests)
 print("PASS Codex model switch reaches actual model request",flush=True)
 browser.until('!document.querySelector("#policy-button").disabled')
 browser.evaluate('document.querySelector("#policy-button").click();document.querySelector("[data-sandbox=workspace-write]").click();true')
 browser.until('document.querySelector("#agent-policy").textContent.includes("工作区写入")')
 browser.until('!document.querySelector("#policy-button").disabled')
 browser.evaluate('document.querySelector("#policy-button").click();document.querySelector("[data-sandbox=read-only]").click();true')
 browser.until('document.querySelector("#agent-policy").textContent.includes("只读")')
 print("PASS Codex native sandbox policy roundtrip",flush=True)
 # 两个独立网页视图回复同一审批；已分发的原生请求只允许一个确认路径。
 target=browser.call("Target.createTarget",{"url":endpoint+"/agent.html"})["targetId"]
 targets=json.loads(urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list").read())
 peer=CDP(next(p for p in targets if p["id"]==target)["webSocketDebuggerUrl"])
 try:
  peer.until('document.querySelectorAll(".agent-session").length===4')
  peer.evaluate("document.querySelector('[data-runtime="+json.dumps(codex["runtimeId"])+"]').click();true")
  peer.until('!document.querySelector("#agent-draft").disabled')
  prompt("APPROVAL_RACE_CODEX")
  browser.until('Boolean(document.querySelector(".agent-approval"))')
  peer.until('Boolean(document.querySelector(".agent-approval"))')
  row=snapshot(codex["runtimeId"]);approval=row["approvals"][0]
  def race_command(allow):
   data={"id":uuid.uuid4().hex,"runtimeId":row["runtimeId"],"sessionId":row["sessionId"],"generation":row["generation"],"action":"respondApproval","requestId":approval["id"],"allow":allow}
   return 'fetch("/api/agents/command",{method:"POST",headers:{"content-type":"application/json"},body:'+json.dumps(json.dumps(data))+'}).then(r=>r.status)'
  status_a=browser.evaluate(race_command(False));status_b=peer.evaluate(race_command(True))
  assert status_a==200 and status_b in [200,409],(status_a,status_b)
  browser.until('!document.querySelector(".agent-approval")')
  peer.until('!document.querySelector(".agent-approval")')
  settled(codex["runtimeId"])
  print("PASS two webpages resolve one native approval and close both views",flush=True)
 finally:
  peer.close();browser.call("Target.closeTarget",{"targetId":target})
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/reset").read()
 prompt("LONG_CODEX")
 browser.until('!document.querySelector("#agent-stop").disabled')
 browser.until('document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_A") && !document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_B")')
 browser.evaluate('document.querySelector("#agent-stop").click();true')
 browser.until('document.querySelector("#agent-state").textContent==="空闲"')
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/release").read()
 print("PASS Codex live first chunk visible before completion and browser stop",flush=True)
 drive({"action":"disconnect_codex"})
 time.sleep(2)
 browser.until('document.querySelector("#agent-state").textContent==="空闲"',timeout=20)
 prompt("WEB_RECONNECTED")
 browser.until('document.querySelector("#agent-draft").value===""')
 assert any("WEB_RECONNECTED" in i["text"] for i in settled(codex["runtimeId"])["items"])
 print("PASS Codex adapter reconnect and same thread history",flush=True)
 browser.call("Network.enable")
 browser.call("Network.emulateNetworkConditions",{"offline":True,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})
 native(901,"LOCAL_OFFLINE");settled(codex["runtimeId"]);time.sleep(1)
 browser.call("Network.emulateNetworkConditions",{"offline":False,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})
 browser.until('document.querySelector("#agent-messages").textContent.includes("LOCAL_OFFLINE")')
 print("PASS native output during Chrome offline and reconnect recovery",flush=True)
 drive({"action":"share","runtimeId":codex["runtimeId"],"shared":True,"control":False})
 browser.until('document.querySelector("#agent-draft").disabled')
 row=snapshot(codex["runtimeId"])
 stale={"id":uuid.uuid4().hex,"runtimeId":row["runtimeId"],"sessionId":row["sessionId"],"generation":row["generation"],"action":"sendText","text":"REJECT_READ_ONLY"}
 status=browser.evaluate('fetch("/api/agents/command",{method:"POST",headers:{"content-type":"application/json"},body:'+json.dumps(json.dumps(stale))+'}).then(r=>r.status)')
 assert status==409,status
 native(901,"LOCAL_READONLY");settled(codex["runtimeId"])
 browser.until('document.querySelector("#agent-messages").textContent.includes("LOCAL_READONLY")')
 drive({"action":"share","runtimeId":row["runtimeId"],"shared":True,"control":True})
 browser.until('!document.querySelector("#agent-draft").disabled')
 print("PASS server rejects read-only operations while native input stays available",flush=True)
 browser.evaluate('document.querySelector("#agent-draft").value="会话草稿";document.querySelector("#agent-draft").dispatchEvent(new Event("input"));location.reload();true')
 browser.until('document.querySelector("#agent-draft").value==="会话草稿"')
 browser.evaluate('document.querySelector("#agent-draft").value="";document.querySelector("#agent-draft").dispatchEvent(new Event("input"));true')
 print("PASS selected session and draft survive reload",flush=True)
 size_before=drive({"action":"size","tab":901})
 for width in [360,390,768,1440]:
  browser.call("Emulation.setDeviceMetricsOverride",{"width":width,"height":850,"deviceScaleFactor":1,"mobile":width<768})
  assert browser.evaluate("document.documentElement.scrollWidth<=window.innerWidth"),width
  shot=browser.call("Page.captureScreenshot",{"format":"png"})
  (ROOT/f"agent-{width}.png").write_bytes(base64.b64decode(shot["data"]))
 assert drive({"action":"size","tab":901})==size_before
 print("PASS 360/390/768/1440 responsive layouts without PTY resize",flush=True)
 # 响应式会话抽屉与真实模型选择面板。
 browser.call("Emulation.setDeviceMetricsOverride",{"width":390,"height":850,"deviceScaleFactor":1,"mobile":True})
 browser.evaluate('document.querySelector("[data-open=sessions]").click();true')
 browser.until('!document.querySelector("#sheet-layer").hidden')
 assert browser.evaluate('getComputedStyle(document.querySelector("#sheet-layer")).justifyContent==="flex-end"')
 browser.evaluate('document.querySelector("[data-close-sheet]").click();document.querySelector("#model-button").click();true')
 browser.until('Boolean(document.querySelector("[data-model]"))')
 shot=browser.call("Page.captureScreenshot",{"format":"png"})
 (ROOT/"agent-model-panel.png").write_bytes(base64.b64decode(shot["data"]))
 browser.evaluate('document.querySelector("[data-close-sheet]").click();true')
 print("PASS mobile session drawer and actual model choices",flush=True)
 # 审批详情固定呈现，两个客户端状态共用真实 request。
 prompt("APPROVAL_DETAILS_CODEX")
 browser.until('Boolean(document.querySelector(".agent-approval"))')
 browser.evaluate('document.querySelector("[data-approval-detail]").click();true')
 browser.until('!document.querySelector("#sheet-layer").hidden')
 shot=browser.call("Page.captureScreenshot",{"format":"png"})
 (ROOT/"agent-approval-panel.png").write_bytes(base64.b64decode(shot["data"]))
 browser.evaluate('document.querySelector("#sheet [data-decision=\\"false\\"]").click();true')
 browser.until('document.querySelector("#sheet-layer").hidden')
 settled(codex["runtimeId"])
 print("PASS approval details use actual pending request",flush=True)
 # 结束一个已托管 Agent，必须保留其 Shell、网页历史及其他 Agent。
 ended=next(r for r in rows if r["tabId"]==903)
 browser.evaluate("document.querySelector('[data-runtime="+json.dumps(ended["runtimeId"])+"]').click();true")
 browser.until('!document.querySelector("#agent-draft").disabled')
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/reset").read()
 prompt("LONG_END_CODEX")
 browser.until('document.querySelector("#agent-state").textContent==="运行中"')
 browser.until('document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_A") && !document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_B")')
 before_items=snapshot(ended["runtimeId"])["items"]
 assert snapshot(ended["runtimeId"])["capabilities"]["end"],"Agent process not registered as owned"
 browser.evaluate('document.querySelector("[data-open=more]").click();document.querySelector("[data-open=end-confirm]").click();true')
 browser.until('Boolean(document.querySelector("[data-end]"))')
 time.sleep(1)
 assert browser.evaluate('!document.querySelector("#sheet-layer").hidden && Boolean(document.querySelector("[data-end]"))'),"运行中结束会话确认框被轮询关闭"
 browser.evaluate('document.querySelector("[data-end]").click();true')
 browser.until('document.querySelector("#agent-state").textContent==="已结束"')
 assert snapshot(ended["runtimeId"])["items"]==before_items
 deadline=time.monotonic()+4
 while True:
  turns=drive({"action":"codex_turns","runtimeId":ended["runtimeId"]})["data"]
  if not any(turn["status"]=="inProgress" for turn in turns):break
  if time.monotonic()>deadline:raise AssertionError("结束会话后 Codex 后台回合仍在运行: "+json.dumps(turns))
  time.sleep(.2)
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/release").read()
 assert drive({"action":"size","tab":903})==[130,38],"Shell ended with Agent"
 native(903,"Write-Output STERM_SHELL_STILL_ALIVE")
 assert snapshot(codex["runtimeId"])["state"]=="idle","Another Agent was terminated"
 print("PASS ending a running Agent stops its backend turn and retains transcript, Shell and other sessions",flush=True)
 # 同一 Shell 再次启动时替换旧记录；关闭运行中标签也要停止共享后台。
 native(903,"codex")
 deadline=time.monotonic()+25
 while True:
  restarted=next((r for r in drive({"action":"list"})["sessions"] if r["tabId"]==903 and r["runtimeId"]!=ended["runtimeId"]),None)
  if restarted and snapshot(restarted["runtimeId"])["sessionId"]:break
  if time.monotonic()>deadline:raise AssertionError("同一 Shell 重启 Agent 超时")
  time.sleep(.2)
 drive({"action":"share","runtimeId":restarted["runtimeId"],"shared":True,"control":True})
 browser.until("Boolean(document.querySelector('[data-runtime="+json.dumps(restarted["runtimeId"])+"]'))")
 assert not browser.evaluate("Boolean(document.querySelector('[data-runtime="+json.dumps(ended["runtimeId"])+"]'))")
 browser.evaluate("document.querySelector('[data-runtime="+json.dumps(restarted["runtimeId"])+"]').click();true")
 browser.until('!document.querySelector("#agent-draft").disabled')
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/reset").read()
 prompt("LONG_CLOSE_CODEX")
 browser.until('document.querySelector("#agent-state").textContent==="运行中"')
 browser.until('document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_A") && !document.querySelector(".agent-item-assistant:last-child")?.textContent.includes("STERM_CODEX_B")')
 thread=snapshot(restarted["runtimeId"])["sessionId"]
 drive({"action":"close","tab":903})
 deadline=time.monotonic()+4
 while True:
  turns=drive({"action":"codex_turns","sessionId":thread})["data"]
  if not any(turn["status"]=="inProgress" for turn in turns):break
  if time.monotonic()>deadline:raise AssertionError("关闭标签后 Codex 后台回合仍在运行")
  time.sleep(.2)
 browser.until("!document.querySelector('[data-runtime="+json.dumps(restarted["runtimeId"])+"]')")
 urllib.request.urlopen(json.loads((ROOT/"mock-endpoint.json").read_text())["endpoint"]+"/hold/release").read()
 assert snapshot(codex["runtimeId"])["state"]=="idle"
 print("PASS restarting Agent replaces old history; closing running tab interrupts only its backend turn",flush=True)
 # 兼容 Windows PowerShell / CMD，结束命令不替换原 Shell。
 for tab_id,shell_kind in [(904,"powershell"),(905,"cmd")]:
  extra=drive({"action":"spawn","tab":tab_id,"agent":"codex","shell":shell_kind,"cwd":str(ROOT/"workspace")})
  row_id=extra["runtimeId"]
  drive({"action":"share","runtimeId":row_id,"shared":True,"control":True})
  deadline=time.monotonic()+25
  while not snapshot(row_id)["sessionId"] and time.monotonic()<deadline:time.sleep(.2)
  browser.until("Boolean(document.querySelector('[data-runtime="+json.dumps(row_id)+"]'))")
  browser.evaluate("document.querySelector('[data-runtime="+json.dumps(row_id)+"]').click();true")
  browser.until('!document.querySelector("#agent-draft").disabled')
  prompt("SHELL_"+shell_kind)
  browser.until('document.querySelector("#agent-draft").value===""')
  settled(row_id)
  assert any(item["text"]=="SHELL_"+shell_kind for item in snapshot(row_id)["items"])
  browser.evaluate('document.querySelector("[data-open=more]").click();document.querySelector("[data-open=end-confirm]").click();document.querySelector("[data-end]").click();true')
  browser.until('document.querySelector("#agent-state").textContent==="已结束"')
  assert drive({"action":"size","tab":tab_id})==[130,38]
  print("PASS "+shell_kind+" direct launch, web input, end and Shell retained",flush=True)
 browser.evaluate("document.querySelector('[data-runtime="+json.dumps(codex["runtimeId"])+"]').click();true")
 browser.until('!document.querySelector("#agent-draft").disabled')
 drive({"action":"revoke"})
 browser.until('!document.querySelector("#login-panel").hidden')
 assert snapshot(codex["runtimeId"])["state"]=="idle"
 print("PASS browser revocation keeps native agent alive",flush=True)
 (ROOT/"chrome-result.json").write_text(json.dumps({"passed":True,"claude":claude["runtimeId"],"codex":codex["runtimeId"]}),encoding="utf8")
except Exception as error:
 print("ERROR "+str(error),flush=True);traceback.print_exc()
 if browser:
  (ROOT/"browser-state.txt").write_text(browser.evaluate("document.body.innerText"),encoding="utf8")
 sys.exit(1)
finally:
 if browser:
  try:browser.call("Browser.close")
  except Exception:pass
  browser.close()
 if CHROME:
  try:CHROME.wait(timeout=10)
  except subprocess.TimeoutExpired:CHROME.terminate();CHROME.wait(timeout=8)
 if log:log.close()
