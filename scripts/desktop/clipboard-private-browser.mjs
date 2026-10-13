import {readFile,writeFile,mkdir,rename} from 'node:fs/promises';
import {join} from 'node:path';
import {createServer} from 'node:http';
// Run by smoke-clipboard-private.ps1. Every browser/probe child belongs to
// a noninteractive window station; CDP input never enters the user's desktop.
const root=process.argv[2];
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,ms=15000){const end=Date.now()+ms;while(Date.now()<end){try{const x=await fn();if(x)return x;}catch{}await sleep(30);}throw Error('Condition timed out');}
async function launch(name,exe,args){const p=join(root,`launch-${name}.json`);await writeFile(p+'.tmp',JSON.stringify({exe,args}));await rename(p+'.tmp',p);return until(async()=>JSON.parse(await readFile(p+'.result','utf8')));}
await until(()=>readFile(join(root,'host-ready.json')));
const server=createServer(async(req,res)=>{try{if(req.url==='/xterm.js'||req.url==='/xterm.css'){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'text/javascript');res.end(await readFile(join(root,req.url.slice(1))));return;}res.setHeader('Content-Type','text/html; charset=utf-8');res.end(`<!doctype html><link rel="stylesheet" href="/xterm.css"><textarea id="edit"></textarea><div id="terminal" style="width:900px;height:500px"></div><script src="/xterm.js"></script><script>window.received='';window.term=new Terminal({cols:80,rows:20});term.open(document.querySelector('#terminal'));term.attachCustomKeyEventHandler(e=>!(e.ctrlKey&&e.code==='KeyV'));document.querySelector('#terminal').addEventListener('paste',e=>{e.preventDefault();e.stopImmediatePropagation();term.paste(e.clipboardData.getData('text/plain'));},true);term.onData(s=>received+=s);</script>`);}catch(e){res.statusCode=500;res.end('Fixture error');}});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const fixture=`http://127.0.0.1:${server.address().port}/`;
const profile=join(root,'browser-profile');
const browser=await launch('browser',process.argv[3] ?? 'C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe',`--user-data-dir="${profile}" --remote-debugging-port=0 --no-first-run --no-default-browser-check --disable-background-networking --disable-sync --disable-extensions --disable-gpu about:blank`);
const [port,path]=await until(async()=>{const s=await readFile(join(profile,'DevToolsActivePort'),'utf8');return s.trim().split('\n');});
const socket=new WebSocket(`ws://127.0.0.1:${port}${path}`);
await new Promise((r,j)=>{socket.addEventListener('open',r,{once:true});socket.addEventListener('error',j,{once:true});});
let id=0;const pending=new Map();
socket.addEventListener('message',e=>{const r=JSON.parse(e.data);if(r.id&&pending.has(r.id)){const p=pending.get(r.id);pending.delete(r.id);clearTimeout(p.timer);r.error?p.reject(Error(JSON.stringify(r.error))):p.resolve(r.result);}});
function send(method,params={},sessionId){return new Promise((resolve,reject)=>{const request={id:++id,method,params,...(sessionId?{sessionId}:{})};const timer=setTimeout(()=>{pending.delete(request.id);reject(Error(`${method} timed out`));},10000);pending.set(request.id,{resolve,reject,timer});socket.send(JSON.stringify(request));});}
const {targetId}=await send('Target.createTarget',{url:fixture});
const {sessionId}=await send('Target.attachToTarget',{targetId,flatten:true});
const evaluate=async expression=>{const r=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true},sessionId);if(r.exceptionDetails)throw Error('Fixture evaluation failed');return r.result.value;};
const receipts=[];const expected='Writer private paste שלום English';
try{
 await send('Page.enable',{},sessionId);await send('Page.bringToFront',{},sessionId);await send('Emulation.setFocusEmulationEnabled',{enabled:true},sessionId);
 await until(()=>evaluate('!!window.term'));
 for(const mode of ['textarea','terminal'])for(const scenario of ['normal','slow-reader','late-reader','early-reader','other-reader','new-copy','browser-text','browser-html','browser-api']){
  const dir=join(root,`${mode}-${scenario}`);await mkdir(dir);
  if(scenario.startsWith('browser-')){
   const expression=scenario==='browser-api'?"navigator.clipboard.writeText('previous private clipboard').then(()=>true)":scenario==='browser-text'?"document.querySelector('#edit').value='previous private clipboard';document.querySelector('#edit').select();document.execCommand('copy')":"document.querySelector('#copy-source')?.remove();document.body.insertAdjacentHTML('beforeend','<p id=copy-source>previous <b>private</b> clipboard</p>');var range=document.createRange();range.selectNodeContents(document.querySelector('#copy-source'));getSelection().removeAllRanges();getSelection().addRange(range);document.execCommand('copy')";
   const copied=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture:true},sessionId);
   if(copied.exceptionDetails||copied.result.value!==true)throw Error(`Browser copy failed: ${scenario}`);
  }
  await evaluate(`(async()=>{getSelection().removeAllRanges();document.activeElement.blur();received='';document.querySelector('#edit').value='';term.reset();await new Promise(r=>term.write('\\x1b[?2004h',r));${mode==='terminal'?'term.focus();':"document.querySelector('#edit').focus();"}})()`);
  await writeFile(join(dir,'config.json'),JSON.stringify({case:scenario,browser_pid:browser.pid}));
  await launch(`${mode}-${scenario}`,join(root,'clipboard-probe.exe'),`--private-source "${dir}"`);
  await until(()=>readFile(join(dir,'ready.json')));
  if(scenario==='slow-reader')await sleep(700);if(scenario==='late-reader')await sleep(3500);
  for(const [type,key,code,vk,modifiers] of [['rawKeyDown','Control','ControlLeft',17,2],['rawKeyDown','v','KeyV',86,2],['keyUp','v','KeyV',86,2],['keyUp','Control','ControlLeft',17,0]])await send('Input.dispatchKeyEvent',{type,key,code,windowsVirtualKeyCode:vk,modifiers},sessionId);
  const observation=await until(async()=>{
   const v=await evaluate(`({value:${mode==='terminal'?"received.replace(/^\\x1b\\[200~/,'').replace(/\\x1b\\[201~$/,'')":"document.querySelector('#edit').value"},helperEmpty:document.querySelector('.xterm-helper-textarea').value===''})`);
   return v.value?{exact:v.value===expected,helperEmpty:v.helperEmpty}:null;
  },5000);
  const receipt=await until(async()=>JSON.parse(await readFile(join(dir,'result.json'),'utf8')));
  const expectedRestore=['normal','slow-reader'].includes(scenario)||scenario.startsWith('browser-');
  const passed=observation.exact&&(!expectedRestore||(receipt.restored&&receipt.clipboard_is_previous&&receipt.formats_restored))&&(!['early-reader','other-reader','late-reader'].includes(scenario)||(!receipt.restored&&receipt.clipboard_is_dictation))&&(scenario!=='new-copy'||receipt.new_copy_kept)&&(mode!=='terminal'||observation.helperEmpty);
  receipts.push({mode,...receipt,received_exact:observation.exact,terminal_helper_empty:observation.helperEmpty,passed});
  await writeFile(join(dir,'done'),'');await until(()=>readFile(join(dir,'cleaned')).then(()=>true));console.log(JSON.stringify(receipts.at(-1)));
  if(!passed)throw Error(`Failed ${mode}/${scenario}`);
 }
 await writeFile(join(root,'brave-receipts.json'),JSON.stringify(receipts,null,2));
}finally{await send('Browser.close').catch(()=>{});socket.close();server.close();await writeFile(join(root,'stop'),'');}
