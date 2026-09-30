import os,subprocess,time,json
binary='/home/zacharyhamm/yinz/.worktrees/agent-cli-verify-more/target/debug/yinz'
env=os.environ.copy();env['YINZ_SOCKET_DIR']='/tmp/yinz-agent-12-14/sockets';env['PATH']=os.path.dirname(binary)+':'+env['PATH']
cmd=[binary,'--session','verify-agent-12-14-fresh']
def run(args):
 p=subprocess.run(cmd+args,env=env,capture_output=True,text=True)
 print(p.stdout,flush=True)
run(['agent','send-keys','terminal_1','Ctrl c','--json'])
run(['agent','recover','terminal_1','--conversation','672d2649-f1ea-4e0d-a653-3a01fccc1c49','--confirm-empty','--json'])
run(['action','paste','--pane-id','terminal_1','What is 4+4? Reply only 8.\u200b'])
time.sleep(2)
run(['agent','send-keys','terminal_1','Enter','--json'])
