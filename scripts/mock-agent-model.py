import os,sys,json,pathlib,threading,time,subprocess,uuid,gzip,zlib,traceback,re
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
sys.stdout.reconfigure(encoding="utf-8")
ROOT=pathlib.Path(sys.argv[1]).resolve()
REQUESTS=[];HOLD=threading.Event();ATTEMPTS={}
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  if self.path=="/hold/release":HOLD.set()
  if self.path=="/hold/reset":HOLD.clear()
  self.reply({"data":[]})
 def reply(self,value):
  raw=json.dumps(value).encode();self.send_response(getattr(self,"reply_status",200));self.send_header("Content-Type","application/json");self.send_header("Content-Length",str(len(raw)));self.end_headers();self.wfile.write(raw)
 def do_POST(self):
  raw=self.rfile.read(int(self.headers.get("Content-Length","0")))
  if self.headers.get("Content-Encoding")=="gzip":raw=gzip.decompress(raw)
  elif self.headers.get("Content-Encoding")=="deflate":raw=zlib.decompress(raw)
  body=json.loads(raw)
  REQUESTS.append(body)
  (ROOT/"mock-requests.json").write_text(json.dumps(REQUESTS,ensure_ascii=False,indent=2),encoding="utf8")
  # 仅本地模拟网络错误，不连接任何实际网关。
  serialized=json.dumps(body)
  retry=re.findall(r"RETRY_[A-Z]+|FAIL_[A-Z]+",serialized)
  if retry:
   marker=retry[-1];ATTEMPTS[marker]=ATTEMPTS.get(marker,0)+1
   if marker.startswith("FAIL_") or ATTEMPTS[marker]<=2:
    if "messages" not in body:
     self.send_response(200);self.send_header("Content-Type","text/event-stream");self.send_header("Connection","close");self.end_headers()
     self.wfile.write(b'event: response.created\ndata: {"type":"response.created","response":{"id":"retry","status":"in_progress","output":[]}}\n\n')
     self.wfile.flush();self.close_connection=True
     return
    self.reply_status=500
    return self.reply({"type":"error","error":{"type":"api_error","message":"STERM isolated retry fixture"}})
  if "/count_tokens" in self.path:return self.reply({"input_tokens":25})
  if "/messages" in self.path:return self.claude(body)
  return self.codex(body)
 def claude(self,body):
  messages=body.get("messages",[]);last=-1;text=""
  for i,m in enumerate(messages):
   if m.get("role")!="user":continue
   c=m.get("content",[]);t=c if isinstance(c,str) else " ".join(x.get("text","") for x in c if x.get("type")=="text")
   found=re.findall("APPROVAL_[A-Z]+|QUESTION_[A-Z]+|SUBAGENT_[A-Z]+|CHILD_[A-Z]+|LONG_[A-Z]+|WEB_[A-Z]+|LOCAL_[A-Z]+|RETRY_[A-Z]+|FAIL_[A-Z]+",t)
   if found:last=i;text=found[-1]
  tool_done=any(c.get("type")=="tool_result" for m in messages[last+1:] for c in (m.get("content",[]) if isinstance(m.get("content",[]),list) else []))
  tool=(text.startswith("APPROVAL_") or text.startswith("QUESTION_") or text.startswith("SUBAGENT_")) and not tool_done
  model=body.get("model","claude-sonnet-4-6");msgid="msg_"+uuid.uuid4().hex
  if not body.get("stream",False):return self.reply({"id":msgid,"type":"message","role":"assistant","model":model,"content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":25,"output_tokens":5}})
  self.send_response(200);self.send_header("Content-Type","text/event-stream");self.end_headers()
  def emit(kind,**data):
   self.wfile.write(("event: "+kind+"\ndata: "+json.dumps({"type":kind,**data})+"\n\n").encode());self.wfile.flush()
  try:
   emit("message_start",message={"id":msgid,"type":"message","role":"assistant","model":model,"content":[],"stop_reason":None,"stop_sequence":None,"usage":{"input_tokens":25,"output_tokens":0}})
   if tool:
    question=text.startswith("QUESTION_")
    child=text.startswith("SUBAGENT_")
    name="Agent" if child else "AskUserQuestion" if question else "Write"
    arguments={"description":"Isolated child task","prompt":"CHILD_CLAUDE"} if child else {"questions":[{"question":"Choose deployment target?","header":"Target","multiSelect":False,"options":[{"label":"A","description":"Alpha"},{"label":"B","description":"Beta"}]},{"question":"Choose optional features?","header":"Features","multiSelect":True,"options":[{"label":"X","description":"First"},{"label":"Y","description":"Second"}]}]} if question else {"file_path":str(ROOT/"workspace"/(text+".txt")),"content":"isolated "+uuid.uuid4().hex}
    item={"type":"tool_use","id":"call_"+uuid.uuid4().hex,"name":name,"input":{}}
    emit("content_block_start",index=0,content_block=item)
    emit("content_block_delta",index=0,delta={"type":"input_json_delta","partial_json":json.dumps(arguments)})
   else:
    emit("content_block_start",index=0,content_block={"type":"text","text":""})
    emit("content_block_delta",index=0,delta={"type":"text_delta","text":"STERM_CLAUDE_A "})
    if text.startswith("LONG_") or text.startswith("CHILD_"):HOLD.wait(20)
    time.sleep(.25)
    emit("content_block_delta",index=0,delta={"type":"text_delta","text":"STERM_CLAUDE_B"})
   emit("content_block_stop",index=0)
   emit("message_delta",delta={"stop_reason":"tool_use" if tool else "end_turn","stop_sequence":None},usage={"output_tokens":10})
   emit("message_stop")
  except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass
 def codex(self,body):
  texts=[]
  for item in body.get("input",[]):
   if item.get("role")=="user":
    c=item.get("content",[]);texts.append(c if isinstance(c,str) else "".join(p.get("text","") for p in c if isinstance(p,dict)))
  last=texts[-1] if texts else ""
  last_user=max((i for i,x in enumerate(body.get("input",[])) if x.get("role")=="user"),default=-1)
  tool=any(marker in last for marker in ["APPROVAL_","QUESTION_","ASYNC_","SUBAGENT_"]) and not any(x.get("type")=="function_call_output" for x in body.get("input",[])[last_user+1:])
  msgid="msg_"+uuid.uuid4().hex
  response={"id":"resp_"+uuid.uuid4().hex,"object":"response","created_at":int(time.time()),"status":"in_progress","model":body.get("model","probe"),"output":[]}
  self.send_response(200);self.send_header("Content-Type","text/event-stream");self.end_headers()
  def emit(kind,**values):
   data={"type":kind,**values};self.wfile.write(("event: "+kind+"\ndata: "+json.dumps(data)+"\n\n").encode());self.wfile.flush()
  try:
   emit("response.created",response=response)
   if tool:
    if "SUBAGENT_" in last:
     toolname="spawn_agent";args={"message":"CHILD_CODEX"}
    elif "ASYNC_" in last:
     toolname="send_message_to_user_async";args={"message":"STERM_ASYNC_NOTICE: please review this update while work continues"}
    elif "QUESTION_" in last:
     toolname="request_user_input";args={"questions":[{"id":"target","header":"Target","question":"Choose deployment target?","options":[{"label":"A","description":"Alpha"},{"label":"B","description":"Beta"}]}]}
    else:
     toolname="exec_command";args={"cmd":"Write-Output STERM_APPROVAL","sandbox_permissions":"require_escalated","justification":"Isolated shared approval verification"}
    item={"id":msgid,"type":"function_call","call_id":"call_"+uuid.uuid4().hex,"name":toolname,"arguments":json.dumps(args),"status":"completed"}
    if toolname=="spawn_agent":item["namespace"]="multi_agent_v1"
    emit("response.output_item.added",output_index=0,item={**item,"arguments":""})
    emit("response.function_call_arguments.delta",item_id=msgid,output_index=0,delta=item["arguments"])
    emit("response.function_call_arguments.done",item_id=msgid,output_index=0,arguments=item["arguments"])
    emit("response.output_item.done",output_index=0,item=item)
   else:
    item={"id":msgid,"type":"message","status":"in_progress","role":"assistant","content":[]}
    emit("response.output_item.added",output_index=0,item=item)
    emit("response.content_part.added",item_id=msgid,output_index=0,content_index=0,part={"type":"output_text","text":"","annotations":[]})
    emit("response.output_text.delta",item_id=msgid,output_index=0,content_index=0,delta="STERM_CODEX_A ")
    if "LONG_" in last or "ASYNC_" in last or "CHILD_" in last:HOLD.wait(20)
    time.sleep(.25)
    emit("response.output_text.delta",item_id=msgid,output_index=0,content_index=0,delta="STERM_CODEX_B")
    item={**item,"status":"completed","content":[{"type":"output_text","text":"STERM_CODEX_A STERM_CODEX_B","annotations":[]}]}
    emit("response.output_text.done",item_id=msgid,output_index=0,content_index=0,text="STERM_CODEX_A STERM_CODEX_B")
    emit("response.output_item.done",output_index=0,item=item)
   emit("response.completed",response={**response,"status":"completed","output":[item],"usage":{"input_tokens":25,"output_tokens":10,"total_tokens":35}})
  except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass
server=ThreadingHTTPServer(("127.0.0.1",0),Handler);server.daemon_threads=True
endpoint=f"http://127.0.0.1:{server.server_port}"
(ROOT/"mock-endpoint.json").write_text(json.dumps({"endpoint":endpoint}),encoding="utf8")
print(endpoint,flush=True)
server.serve_forever()
