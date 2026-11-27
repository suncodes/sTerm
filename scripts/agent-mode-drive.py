import json,pathlib,time,sys,uuid
root=pathlib.Path(sys.argv[1]).resolve()/"appdata"
data=json.loads(sys.argv[2]);data["id"]=uuid.uuid4().hex
(root/"probe-command.json").write_text(json.dumps(data,ensure_ascii=False),encoding="utf-8")
deadline=time.monotonic()+45
while time.monotonic()<deadline:
 try:
  result=json.loads((root/"probe-result.json").read_text(encoding="utf-8-sig"))
  if result.get("id")==data["id"]:
   print(json.dumps(result,ensure_ascii=False));sys.exit(0 if result.get("ok") else 1)
 except (FileNotFoundError,json.JSONDecodeError):pass
 time.sleep(.1)
raise RuntimeError("native probe timed out")
