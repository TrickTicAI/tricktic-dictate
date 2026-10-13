// Runs the inherited Android smoke against an isolated, explicitly fake Writer API.
// No provider/model, keys, user audio, or existing service is used.
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
const root = await mkdtemp(join(homedir(), '.local/share/metrix-writer/mw56-fixture-'));
const wav = Buffer.alloc(44 + 32000);
wav.write('RIFF'); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVEfmt ', 8);
wav.writeUInt32LE(16,16); wav.writeUInt16LE(1,20); wav.writeUInt16LE(1,22);
wav.writeUInt32LE(16000,24); wav.writeUInt32LE(32000,28); wav.writeUInt16LE(2,32); wav.writeUInt16LE(16,34); wav.write('data',36); wav.writeUInt32LE(32000,40);
for(let i=0;i<16000;i++) wav.writeInt16LE(Math.round(1000*Math.sin(i*2*Math.PI*440/16000)),44+i*2);
const fixture = join(root,'generated-tone.wav'); await writeFile(fixture,wav);
process.env.WRITER_ANDROID_PUBLIC_WAV=fixture;
process.env.WRITER_ANDROID_FIXTURE_API='1';
process.env.WRITER_ANDROID_SMOKE=join(root,'results');
const token = `mw1.${randomUUID()}.${randomUUID().replaceAll('-','')}${randomUUID().replaceAll('-','')}`;
process.env.WRITER_ANDROID_FIXTURE_CREDENTIAL = token;
execFileSync(fileURLToPath(new URL('../gradlew', import.meta.url)), ['-p', fileURLToPath(new URL('..', import.meta.url)), '--offline', '-PwriterTestRunner=com.metrix.writer.MobileHttpsRunner', ':app:assembleDevDebug', ':app:assembleDevDebugAndroidTest', ':editor:assembleDebug'], {stdio:'pipe'});
const recordings=new Map(), jobs=new Map(), requests=new Map();
let control={};
const events=[];
const api=createServer(async(req,res)=>{
 events.push({at:Date.now(),method:req.method,path:req.url,control:{...control}});
 const send=(data,code=200)=>{res.writeHead(code,{'Content-Type':'application/json'});res.end(JSON.stringify({api_version:'1',data}));};
 try {
  if(req.url.startsWith('/api/v1/') && req.headers.authorization !== `Bearer ${token}`) {send({},401);return;}
  if(req.url==='/api/v1/mobile/connection') {send({service:'metrix-writer-mobile',mobile_version:1,device_id:token.split('.')[1],models:[{selection:{provider:'local',model:'ivrit-turbo'},input_languages:['he'],execution_location:'self_hosted_server',configured:true,available:true,status:'available'},{selection:{provider:'openai',model:'gpt-transcribe'},input_languages:['he','en','mixed'],execution_location:'cloud',configured:true,available:false,status:'unverified'}]});return;}
  if(req.method==='POST' && req.url==='/api/v1/recordings') {
   const boundary=req.headers['content-type'].split('boundary=')[1];let chunks=[];
   req.on('data',chunk=>{chunks.push(chunk);const all=Buffer.concat(chunks);const text=all.toString();const start=text.indexOf('\r\n\r\n')+4;const end=text.indexOf('\r\n--'+boundary,start);
    if(start>=4&&end>start){const metadata=JSON.parse(text.slice(start,end)).data;if(!recordings.has(metadata.id)) recordings.set(metadata.id,{metadata,audio:{state:'uploading'}});}
   });
   req.on('end',()=>{const all=Buffer.concat(chunks);const text=all.toString();const start=text.indexOf('\r\n\r\n')+4;const end=text.indexOf('\r\n--'+boundary,start);const metadata=JSON.parse(text.slice(start,end)).data;
    const audioStart=all.indexOf(Buffer.from('\r\n\r\n'),end)+4;const audioEnd=all.lastIndexOf(Buffer.from('\r\n--'+boundary));const bytes=all.subarray(audioStart,audioEnd);
    recordings.set(metadata.id,{metadata,audio:{state:'saved'},bytes});send({metadata,audio:{state:'saved'}});
   }); req.on('error',()=>{}); return;
  }
  const chunks=[];for await(const chunk of req) chunks.push(chunk);const body=chunks.length?JSON.parse(Buffer.concat(chunks)):null;
  if(req.url==='/fixture/control'){control=body;send({ok:true});return;}
  if(req.method==='POST'&&req.url==='/api/v1/transcriptions'){
   const request=body.data;let job=requests.get(request.idempotency_key);
   if(!job){const id=randomUUID();const fail=request.options.selection.provider==='openai';job={id,request,status:fail?{state:'failed',error:{message:'Fixture provider deliberately unavailable'}}:{state:'succeeded',result:{transcript:{raw_text:'Fixture original עברית English'}}}};requests.set(request.idempotency_key,job);jobs.set(id,job);}
   if(control.drop){req.socket.destroy();return;}
   if(control.delay){const delay=control.delay;control.delay=0;setTimeout(()=>send(job),delay);return;}
   send(job);return;
  }
  if(req.url.startsWith('/api/v1/jobs/')){send(jobs.get(req.url.split('/').at(-1)));return;}
  const match=req.url.match(/^\/api\/v1\/recordings\/([^/]+)(\/audio)?$/);
  if(match){const recording=recordings.get(match[1]);if(!recording){send({},404);return;}if(match[2]){res.end(recording.bytes);return;}send({metadata:recording.metadata,audio:recording.audio});return;}
  send({},404);
 } catch { if(!res.headersSent)send({},500);else res.end(); }
});
await new Promise((resolve,reject)=>{api.once('error',reject);api.listen(18795,'127.0.0.1',resolve);});
console.log(JSON.stringify({fixtureApiPid:process.pid,port:18795,output:root,realProvider:false}));
try { await import('./smoke.mjs'); } finally { await writeFile(join(root,'api-events.json'),JSON.stringify(events,null,2)); api.closeAllConnections(); await new Promise(resolve=>api.close(resolve)); }
