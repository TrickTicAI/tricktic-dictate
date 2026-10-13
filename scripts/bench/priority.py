#!/usr/bin/env python3
"""Exercise actual worker preemption using two existing WAVs. Raw output is private."""
import argparse,json,subprocess,time
from pathlib import Path
from urllib.parse import urlparse
p=argparse.ArgumentParser();p.add_argument('--url',required=True);p.add_argument('--preview',type=Path,required=True);p.add_argument('--final',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--repeats',type=int,default=5);args=p.parse_args()
u=urlparse(args.url)
if u.scheme!='http' or u.hostname!='127.0.0.1' or u.path!='/inference' or u.username or u.query:p.error('Use a credential-free local worker inference URL')
root=Path(__file__).resolve().parents[2]
if any(p.resolve().is_relative_to(root) for p in [args.preview,args.final,args.output]):p.error('Keep audio/output outside source')
args.output.mkdir(parents=True,exist_ok=False)
def native(path):return subprocess.check_output(['wslpath','-w',str(path.resolve())],text=True).strip()
curl='/mnt/c/Windows/System32/curl.exe'
def request(path,label,preview=False):
 cmd=[curl,'--silent','--show-error','--noproxy','*','--max-time','180','--output',native(args.output/(label+'.json')),'--write-out','%{http_code} %{time_total}','--form','file=@'+native(path)]
 for k,v in dict(language='he',response_format='json',translate='false',temperature=0,temperature_inc=.2,best_of=2,beam_size=-1,audio_ctx=768,no_timestamps='true',writer_preview='true' if preview else 'false').items():cmd+=['--form-string',f'{k}={v}']
 return cmd+[args.url]
rows=[]
for trial in range(args.repeats):
 before=request(args.preview,f'preview-{trial}',True);after=request(args.final,f'final-{trial}')
 a=subprocess.Popen(before,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True);time.sleep(.1)
 start=time.perf_counter()
 # The real adapter must signal before awaiting its own provider mutex.
 signal=subprocess.run([curl,'--silent','--show-error','--noproxy','*','--max-time','5','--data','',args.url.replace('/inference','/writer/finalize')],capture_output=True,text=True,timeout=10)
 assert signal.returncode==0 and json.loads(signal.stdout)['acknowledged']
 pre,err=a.communicate(timeout=185);assert a.returncode==0
 result=subprocess.run(after,capture_output=True,text=True,timeout=185);assert result.returncode==0
 duration=(time.perf_counter()-start)*1000
 pre_code,pre_time=pre.split();code,seconds=result.stdout.split()
 assert pre_code=='409',f'Preview did not abort: {pre_code}'
 assert code=='200',f'Final failed: {code}'
 row={'trial':trial,'stop_to_result_ms':duration,'preview_ms':float(pre_time)*1000,'final_http_ms':float(seconds)*1000,'preview_status':int(pre_code),'final_status':int(code)}
 rows.append(row);print(json.dumps(row),flush=True)
(args.output/'summary.json').write_text(json.dumps({'scope':'Real cancellation + local provider wait + final request; includes Windows curl process overhead, excludes app capture/persistence/insertion','runs':rows},indent=2))
