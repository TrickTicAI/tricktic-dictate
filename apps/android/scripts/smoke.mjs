// Headless emulator smoke: silent AudioRecord recovery plus separate public saved-WAV inference.
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { request as httpRequest } from 'node:http';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';
const sdk=process.env.ANDROID_HOME||process.env.WRITER_ANDROID_SDK||join(homedir(),'Android/Sdk');
const output=process.env.WRITER_ANDROID_SMOKE||join(homedir(),'.local/share/metrix-writer/android-smoke-'+Date.now());
const fixture=process.env.WRITER_ANDROID_PUBLIC_WAV;
if (!fixture) throw Error('Set WRITER_ANDROID_PUBLIC_WAV to an existing public/consented WAV outside source');
await mkdir(output,{recursive:false});
const serial=process.env.WRITER_ANDROID_SERIAL;
if (!serial) throw Error('Set WRITER_ANDROID_SERIAL explicitly');
const pkg='com.metrix.writer.debug'; const component=pkg+'/com.metrix.writer.MainActivity';
const server='http://127.0.0.1:18795';
if(process.env.WRITER_ANDROID_FIXTURE_API!=='1') throw Error('Run fixture-smoke.mjs; authenticated real-API pairing is covered by mobile-proof.py.');
const deviceToken=process.env.WRITER_ANDROID_FIXTURE_CREDENTIAL;
const authorizedFetch=(url,options={})=>fetch(url,{...options,headers:{...options.headers,Authorization:`Bearer ${deviceToken}`}});
const hash=b=>createHash('sha256').update(b).digest('hex');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function adb(...args) { return await new Promise((resolve,reject)=> { const child=spawn(join(sdk,'platform-tools/adb'+(process.platform==='win32'?'.exe':'')),['-s',serial,...args],{windowsHide:true,stdio:['ignore','pipe','pipe']}); const out=[];let err='';child.stdout.on('data',b=>out.push(b));child.stderr.on('data',b=>err+=b);child.on('error',reject);child.on('exit',c=>c===0?resolve(Buffer.concat(out)):reject(Error("ADB command failed: "+args.slice(0,5).join(" ")+" "+err))); }); }
async function pairFixture(selection='local/ivrit-turbo') {
 const config=JSON.stringify({origin:'http://10.0.2.2:18795',token:deviceToken,selection});
 await new Promise((resolve,reject)=>{
  const child=spawn(join(sdk,'platform-tools/adb'),['-s',serial,'shell','run-as',pkg,'sh','-c', "'mkdir -p no_backup && cat > no_backup/mobile-proof.json'"],{stdio:['pipe','pipe','pipe']});
  child.on('error',reject);child.on('exit',code=>code===0?resolve():reject(Error('Could not provision private fixture input')));child.stdin.end(config);
 });
 const result=(await adb('shell','am','instrument','-w','-e','phase','fixture-pair',pkg+'.test/com.metrix.writer.MobileHttpsRunner')).toString();
 if(!result.includes('PASS:'))throw Error('Fixture pairing failed');
}
async function ui() { await adb('shell','uiautomator','dump','/data/local/tmp/writer-ui.xml');return (await adb('exec-out','cat','/data/local/tmp/writer-ui.xml')).toString(); }
async function tap(text) {
 for(let i=0;i<8;i++) { const xml=await ui(); const nodes=xml.match(/<node\b[^>]*>/g)||[]; const node=nodes.find(n=>new RegExp(`text="${text}"`,'i').test(n));
  if(node) { const b=node.match(/bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/); if(b && Number(b[3])>Number(b[1]) && Number(b[4])>Number(b[2])) { await adb('shell','input','tap',String(Math.floor((+b[1]+ +b[3])/2)),String(Math.floor((+b[2]+ +b[4])/2)));return; } }
  await adb('shell','input','swipe','400','1600','400','400','250');
 } throw Error('Button unavailable: '+text);
}
async function launch() { await adb('shell','am','start','-W','-n',component); await sleep(1200); }
async function sidecars() {return (await adb('shell','run-as',pkg,'ls','files/recordings')).toString().trim().split(/\s+/).filter(n=>n.endsWith('.json'));}
async function readSession(file) { return JSON.parse((await adb('exec-out','run-as',pkg,'cat','files/recordings/'+file)).toString()); }
async function waitUntil(check, ms=30000) { const start=Date.now(); while(Date.now()-start<ms){const result=await check();if(result)return result;await sleep(500);}throw Error('Smoke condition timed out'); }
if (!serial.startsWith('emulator-') || !(await adb('emu','avd','name')).toString().startsWith('Writer_')) throw Error('Smoke requires an explicitly selected Writer-owned emulator');
await adb('shell','wm','size','1080x1920');await adb('shell','wm','density','320');
await adb('install','-r',fileURLToPath(new URL('../app/build/outputs/apk/dev/debug/app-dev-debug.apk',import.meta.url)));
await adb('install','-r',fileURLToPath(new URL('../app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk',import.meta.url)));
await adb('install','-r',fileURLToPath(new URL('../editor/build/outputs/apk/debug/editor-debug.apk',import.meta.url)));
await pairFixture();
if (!(await adb('shell','dumpsys','package',pkg)).toString().includes('android.permission.RECORD_AUDIO: granted=true')) throw Error('Allow microphone access through Writer onboarding before running the companion fixture smoke');
let previousNames; try { previousNames = new Set(await sidecars()); } catch { previousNames = new Set(); }
await launch(); await tap('Start recording');await sleep(3500);
await adb('shell','am','force-stop',pkg);
const capturedName=(await sidecars()).find(name=>!previousNames.has(name)); if(!capturedName) throw Error('No new capture created'); const before=await readSession(capturedName);
const wavName=before.id+'.wav';const captured=await adb('exec-out','run-as',pkg,'cat','files/recordings/'+wavName);
if(captured.length<=44||before.capture_state!=='capturing')throw Error('No recoverable in-progress capture');
await launch(); const recovered=await waitUntil(async()=>{const s=await readSession(capturedName);return s.capture_state==='interrupted'?s:null;});
const after=await adb('exec-out','run-as',pkg,'cat','files/recordings/'+wavName);
if(recovered.capture_state!=='interrupted'||hash(captured.subarray(44))!==hash(after.subarray(44))||recovered.byte_length!==after.length)throw Error('Capture recovery changed PCM/lost metadata');
await writeFile(join(output,'synthetic-capture.wav'),after);
// The service keeps one capture across HOME; only explicit Stop finalizes it.
const beforeLifecycle = new Set(await sidecars());
await tap('Start recording'); await sleep(1200);
await adb('shell','input','keyevent','KEYCODE_HOME');
const lifecycleName=(await sidecars()).find(name=>!beforeLifecycle.has(name));
await sleep(1200);
if((await readSession(lifecycleName)).capture_state!=='capturing')throw Error('HOME unexpectedly ended capture');
await launch(); await tap('Stop recording');
const lifecycleSession=await waitUntil(async()=>{const s=await readSession(lifecycleName);return s.capture_state==='complete'?s:null;});
if(lifecycleSession.byte_length<=44)throw Error('Explicit Stop after HOME lost audio');
// Recreate the Activity during another capture; its service must keep the same writer.
const beforeRotation = new Set(await sidecars());await tap('Start recording');await sleep(1200);
await adb('shell','settings','put','system','accelerometer_rotation','0');await adb('shell','settings','put','system','user_rotation','1');
const rotationName=(await sidecars()).find(name=>!beforeRotation.has(name));
await sleep(1200);
if((await readSession(rotationName)).capture_state!=='capturing')throw Error('Rotation unexpectedly ended capture');
await adb('shell','settings','put','system','user_rotation','0');await sleep(2000);
await tap('Stop recording');
const rotationSession=await waitUntil(async()=>{const s=await readSession(rotationName);return s.capture_state==='complete'?s:null;});
if(rotationSession.byte_length<=44)throw Error('Stop after rotation lost audio');
// Stop submits the newly saved native recording without a second Transcribe tap.
const beforeAuto = new Set(await sidecars());
await tap('Start recording'); await sleep(800); await tap('Stop and transcribe');
const autoName=(await sidecars()).find(name=>!beforeAuto.has(name));
const autoSession=await waitUntil(async()=>{const s=await readSession(autoName);return s.attempts?.length===1&&s.last_timing?.job_id===s.attempts[0].id?s:null;},180000);
if(autoSession.capture_state!=='complete'||autoSession.attempts[0].request.recording_id!==autoSession.id)throw Error('Stop did not submit the exact saved recording');
await adb('shell','am','force-stop',pkg);
const requestToResultMs=[];
const bytes=await readFile(fixture); const id=randomUUID(); const duration=(bytes.length-44)*1000/32000;
const metadata={id,created_at_ms:Date.now(),mime_type:'audio/wav',byte_length:bytes.length,duration_ms:Math.round(duration),input_language:'he',capture_state:'complete'};
const privateSidecar=join(output,id+'.json');await writeFile(privateSidecar,JSON.stringify({...metadata,attempts:[]}));
await adb('push',fixture,'/data/local/tmp/writer-fixture.wav');await adb('push',privateSidecar,'/data/local/tmp/writer-fixture.json');
await adb('shell','run-as',pkg,'cp','/data/local/tmp/writer-fixture.wav','files/recordings/'+id+'.wav');
await adb('shell','run-as',pkg,'cp','/data/local/tmp/writer-fixture.json','files/recordings/'+id+'.json');
// Leave a real partially uploaded recording on the isolated API, then let Android repair it.
const boundary='writer-smoke-'+randomUUID();
await new Promise(resolve=>{const req=httpRequest(server+'/api/v1/recordings',{method:'POST',headers:{'Content-Type':'multipart/form-data; boundary='+boundary,Authorization:`Bearer ${deviceToken}`} },res=>res.resume());req.on('error',()=>{});req.write('--'+boundary+'\r\nContent-Disposition: form-data; name="metadata"\r\n\r\n'+JSON.stringify({api_version:'1',data:metadata})+'\r\n--'+boundary+'\r\nContent-Disposition: form-data; name="audio"; filename="recording.wav"\r\nContent-Type: audio/wav\r\n\r\n');req.write(bytes.subarray(0,20));setTimeout(()=>{req.destroy();resolve();},500);});
await waitUntil(async()=>{const r=await authorizedFetch(server+'/api/v1/recordings/'+id);return r.ok&&(await r.json()).data.audio.state==='uploading';});
await launch(); const latencies=[];
for(let attempt=0;attempt<2;attempt++) {
 const start=Date.now();await tap('Transcribe / retry saved audio');
 const saved=await waitUntil(async()=>{const s=await readSession(id+'.json');return s.attempts?.length===attempt+1&&s.last_timing?.job_id===s.attempts[attempt].id?s:null;},180000);latencies.push(Date.now()-start);
 requestToResultMs.push(saved.last_timing.request_to_result_ms);
 const job=saved.attempts[attempt];if(job.status.state!=='succeeded')throw Error('Server transcription failed');
 if(job.request.recording_id!==id||job.request.options.selection.provider!=='local'||job.request.options.input_language!=='he')throw Error('Wrong source/provider/language');
 if(attempt&&job.request.retry_of_job_id!==saved.attempts[0].id)throw Error('Retry lost previous job');
 const retained=Buffer.from(await(await authorizedFetch(server+'/api/v1/recordings/'+id+'/audio')).arrayBuffer());if(hash(retained)!==hash(bytes))throw Error('Retained source changed');
 const xml=await ui(); if(!xml.includes('Transcription succeeded'))throw Error('Success was not visible in app');
 const originalNode=(xml.match(/<node\b[^>]*>/g)||[]).find(n=>n.includes('content-desc="Original transcript"'));
 const decoded=originalNode?.match(/text="([^"]*)"/)?.[1].replace(/&#(\d+);/g,(_,n)=>String.fromCodePoint(Number(n))).replace(/&quot;/g,'"').replace(/&apos;/g,"'").replace(/&lt;/g,'<').replace(/&gt;/g,'>').replace(/&amp;/g,'&');
 if(decoded?.trim()!==job.status.result.transcript.raw_text.trim())throw Error('Original text not visible unchanged');
 await writeFile(join(output,'session-'+attempt+'.json'),JSON.stringify(saved,null,2));
 await adb('shell','am','force-stop',pkg);await launch();
}
// A real unconfigured cloud attempt fails independently of saved audio/originals.
await adb('shell','am','force-stop',pkg); await pairFixture('openai/gpt-transcribe'); await launch(); await tap('Transcribe / retry saved audio');
const failed=await waitUntil(async()=>{const s=await readSession(id+'.json');return s.attempts?.length===3?s:null;});
if(failed.attempts[2].status.state!=='failed'||!failed.attempts[2].status.error.message)throw Error('Missing-provider attempt did not fail explicitly');
if(failed.attempts[0].status.result.transcript.raw_text!==failed.attempts[1].status.result.transcript.raw_text)throw Error('Successful originals changed');
const failedUi=await ui();if(!failedUi.includes('audio and previous originals retained'))throw Error('Failure retention guidance not visible');
await writeFile(join(output,'session-failed.json'),JSON.stringify(failed,null,2));
// Fixture-only process/screen tests: exact retry identity and durable screen-independent results.
if(process.env.WRITER_ANDROID_FIXTURE_API==='1') {
 await adb('shell','am','force-stop',pkg); await pairFixture(); await launch();
 await authorizedFetch(server+'/fixture/control',{method:'POST',body:JSON.stringify({drop:true})});
 await tap('Transcribe / retry saved audio');
 const uncertain=await waitUntil(async()=>{const item=await readSession(id+'.json');return item.pending&&item.last_status?.includes('exact pending request are saved')?item:null;});
 await sleep(1000); await adb('shell','am','force-stop',pkg); await launch();
 const restored=await readSession(id+'.json');
 if(JSON.stringify(restored.pending)!==JSON.stringify(uncertain.pending)||restored.pending_server!==uncertain.pending_server)throw Error('Uncertain identity changed across process death');
 await authorizedFetch(server+'/fixture/control',{method:'POST',body:JSON.stringify({delay:8000})});
 await tap('Transcribe / retry saved audio');
 await adb('shell','am','start','-W','-n','com.metrix.writer.editor/.EditorActivity');
 const delivered=await waitUntil(async()=>{const item=await readSession(id+'.json');return item.attempts.length===4&&item.delivery?.state==='pending'?item:null;});
 if(JSON.stringify(delivered.attempts[3].request)!==JSON.stringify(uncertain.pending))throw Error('Retry did not use the exact saved request');
 if(hash(await adb('exec-out','run-as',pkg,'cat','files/recordings/'+id+'.wav'))!==hash(bytes))throw Error('Failure/retry changed WAV');
 const editorUi=await ui();if(!editorUi.includes('Native editor'))throw Error('Independent editor fixture did not launch');
 await launch();
}
const result={passed:true,emulator:serial,headless:true,hostAudioEnabled:false,syntheticCapture:{bytes:after.length,duration_ms:recovered.duration_ms,pcmPreserved:true,backgroundCaptureContinued:true,rotationCaptureContinued:true,stopSubmittedSavedAudio:true,stopToResultMs:autoSession.last_timing.stop_to_result_ms,sha256:hash(after)},savedAudio:{fixtureProvider:process.env.WRITER_ANDROID_FIXTURE_API==='1',audio_sha256:hash(bytes),recording_id:id,attempts:2,repairedInterruptedUpload:true,unconfiguredProviderFailurePreservedOriginal:true,submitToPersistedMs:latencies,requestToResultMs,execution:'self_hosted_server'},uncertainRequestProcessRecovery:process.env.WRITER_ANDROID_FIXTURE_API==='1',resultSavedWithoutActiveScreen:process.env.WRITER_ANDROID_FIXTURE_API==='1',physicalMicrophone:false,onDeviceInference:false};
await writeFile(join(output,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify({...result,output}));
