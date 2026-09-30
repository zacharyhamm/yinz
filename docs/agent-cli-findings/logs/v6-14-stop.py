import json,sys,time
from pathlib import Path
hook=json.load(sys.stdin)
root=Path('/tmp/yinz-agent-12-14')
message=hook.get('last_assistant_message','')
with (root/'stop-events.jsonl').open('a') as f: f.write(json.dumps(hook)+'\n')
if 'BLOCKME_1214' in message and not (root/'blocked').exists():
 (root/'blocked').touch()
 time.sleep(6)
 print(json.dumps({'decision':'block','reason':'Continue this same turn, and reply only FINAL_1214.'}))
