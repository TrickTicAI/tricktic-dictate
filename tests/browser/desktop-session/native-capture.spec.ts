import { expect, test } from '@playwright/test';

test.beforeEach(async ({page}) => {
  await page.route('http://tauri.localhost/**', async r => { const response = await r.fetch({url:r.request().url().replace('http://tauri.localhost','http://127.0.0.1:15184')}); await r.fulfill({response}); });
  await page.route('http://tauri.localhost/native-test',r=>r.fulfill({contentType:'text/html',body:'<!doctype html><title>Native capture adapter test</title>'}));
  await page.goto('http://tauri.localhost/native-test');
  await page.evaluate(async () => {
    const w=window as any;
    w.calls=[];w.native={active:false,id:null,elapsed:0,saved_bytes:0,error:null,rms:.2,peak:.3,level_sequence:1};
    w.__TAURI__={core:{invoke:async(command:string,args:any)=>{
      w.calls.push(command);
      if(command==='capture_start') { await new Promise(resolve=>{w.allowStart=resolve;});w.native={...w.native,active:true,id:args.id};return {...w.native}; }
      if(command==='capture_stop'){w.native={...w.native,active:false,elapsed:1000,saved_bytes:32080};return {...w.native};}
      if(command==='capture_status'){const status={...w.native};if(w.delayPoll)await new Promise(resolve=>{w.releasePoll=resolve;});return status;}
      if(command==='capture_list')return [];
      throw new Error('Unexpected native command');
    }}};
    Object.defineProperty(navigator,'mediaDevices',{value:{getUserMedia:()=>{throw new Error('Browser microphone must not be used.');}}});
    const path='http://tauri.localhost/src/features/desktop-session/runtime/native.ts';
    const {NativeCapture}=await import(path); w.capture=new NativeCapture(); w.startNative=async (language:string)=>{w.capture.start(language);while(!w.allowStart)await new Promise(r=>setTimeout(r,0));w.allowStart();};
  });
});
test('stop during a pending native start drains exactly once without opening browser audio',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.started=w.capture.start('mixed',crypto.randomUUID());});
  await expect.poll(()=>page.evaluate(()=>(window as any).calls)).toContain('capture_start');
  await page.evaluate(()=>{const w=window as any;w.stopped=w.capture.stopAndDrain();w.capture.stop();w.allowStart();});
  const result=await page.evaluate(async()=>{const w=window as any;await w.stopped;return {calls:w.calls,snapshot:w.capture.snapshot};});
  expect(result.calls.filter((s:string)=>s==='capture_stop')).toHaveLength(1);
  expect(result.snapshot).toMatchObject({state:'idle',savedBytes:32080,error:null});
});
test('concurrent stop requests share one drain and stale native levels are not re-emitted',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.levels=0;w.capture.subscribeLevel(()=>w.levels++);void w.startNative('en');});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot.state)).toBe('recording');
  await expect.poll(()=>page.evaluate(()=>(window as any).calls.filter((s:string)=>s==='capture_status').length)).toBeGreaterThan(2);
  expect(await page.evaluate(()=>(window as any).levels)).toBe(1);
  await page.evaluate(async()=>{const w=window as any;await Promise.all([w.capture.stopAndDrain(),w.capture.stopAndDrain()]);});
  expect(await page.evaluate(()=>(window as any).calls.filter((s:string)=>s==='capture_stop').length)).toBe(1);
});
test('native interruption publishes retained bytes and returns to idle',async({page})=>{
  await page.evaluate(()=>{const w=window as any;void w.startNative('he');});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot.state)).toBe('recording');
  await page.evaluate(()=>{const w=window as any;w.native={...w.native,active:false,saved_bytes:32080,error:'Device disconnected.'};});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot)).toMatchObject({state:'idle',savedBytes:32080,error:'Device disconnected.'});
});

test('a delayed status reply cannot resurrect recording after stop',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.delayPoll=true;void w.startNative('en');});
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).releasePoll)).toBe('function');
  await page.evaluate(async()=>{const w=window as any;await w.capture.stopAndDrain();w.releasePoll();});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot.state)).toBe('idle');
  expect(await page.evaluate(()=>(window as any).capture.snapshot.savedBytes)).toBe(32080);
});

test('an open microphone still sending silence is recording but not listening; older hosts count as listening',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.native={...w.native,listening:false};void w.startNative('en');});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot)).toMatchObject({state:'recording',listening:false});
  await page.evaluate(()=>{const w=window as any;w.native={...w.native,listening:true};});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot.listening)).toBe(true);
  await page.evaluate(()=>{const w=window as any;const {listening,...older}=w.native;w.native={...older,listening:undefined};});
  await expect.poll(()=>page.evaluate(()=>(window as any).capture.snapshot.listening)).toBe(true);
});
