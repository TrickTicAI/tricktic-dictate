import { test, expect } from '@playwright/test';
import { stubOwnerSession, loginFixture } from '../auth/tests/fixture';

test('saved Blob hashes match WebCrypto at SHA padding and stream boundaries', async ({ page }) => {
  await page.route('**/api/v1/**',route=>route.fulfill({status:503,body:'offline fixture'}));
  await stubOwnerSession(page); await page.goto('/'); await loginFixture(page);
  const results = await page.evaluate(async () => {
    const path='/src/features/transcripts/hash.ts'; const { audioDigest } = await import(path);
    const results=[];
    for(const length of [0,1,55,56,63,64,65,65535,65536,65537,1024*1024+17]) {
      const bytes=new Uint8Array(length).map((_,i)=>(i*137)%251);
      const expected=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',bytes)),b=>b.toString(16).padStart(2,'0')).join('');
      results.push([await audioDigest(new Blob([bytes])),expected]);
    }
    return results;
  });
  for(const [actual,expected] of results) expect(actual).toBe(expected);
});

test('confirmed chunks and final receipt survive lost responses and page reload without duplicate bytes', async ({ page }) => {
  await page.route('**/api/v1/**',route=>route.fulfill({status:503,body:'offline fixture'}));
  await stubOwnerSession(page); await page.goto('/'); await loginFixture(page);
  const parts:Buffer[]=[]; const offsets:number[]=[]; let session='11111111-1111-4111-8111-111111111111';
  let digest=''; let metadata:any; let saved=false; let fail=true; let count=0; let finishCount=0;
  await page.route('**/api/v1/recordings/**',async route=> {
    const req=route.request(), path=new URL(req.url()).pathname;
    const cp=()=>({session_id:session,offset:count,expires_at_ms:Date.now()+60000,saved,sha256:digest});
    const reply=(data:any)=>route.fulfill({json:{api_version:'1',data}});
    if(path.endsWith('/upload/finish')) { finishCount++; saved=true; return route.abort('connectionreset'); }
    if(path.endsWith('/upload')) {
      if(req.method()==='POST') { const body=req.postDataJSON().data; metadata=body.metadata; digest=body.sha256; return reply(cp()); }
      const offset=Number(req.headers()['upload-offset']); expect(offset).toBe(count); expect(req.headers()['upload-session']).toBe(session);
      const bytes=req.postDataBuffer()!; offsets.push(offset); parts.push(bytes); count+=bytes.length;
      if(fail) { fail=false; return route.abort('connectionreset'); } return reply(cp());
    }
    return route.fulfill({status:404,json:{api_version:'1',error:{message:'missing'}}});
  });
  const upload = () => page.evaluate(async () => {
    const path='/src/features/transcripts/api.ts'; const { ensureUploaded }=await import(path);
    const bytes=new Uint8Array(2500000).map((_,i)=>i%251);
    const metadata={id:'22222222-2222-4222-8222-222222222222',created_at_ms:1,duration_ms:2,byte_length:bytes.length,mime_type:'audio/wav',input_language:'mixed',capture_state:'complete'};
    const progress:string[]=[];
    await ensureUploaded(metadata,async()=>new Blob([bytes]),(message:string)=>progress.push(message));
    return progress;
  });
  const progress=await upload();
  expect(progress.some(p=>p.includes('bytes confirmed'))).toBe(true);
  expect(offsets).toEqual([0,1048576,2097152]); expect(finishCount).toBe(1);
  expect(Buffer.concat(parts)).toEqual(Buffer.from(Uint8Array.from({length:2500000},(_,i)=>i%251)));
  await page.reload(); await loginFixture(page); await upload();
  expect(finishCount).toBe(1); expect(count).toBe(metadata.byte_length);
});
