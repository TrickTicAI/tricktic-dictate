import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { signer } from './update-metadata-fixture.mjs';
import { manifestFor, verifyMetadata, writeReleaseMetadata, writeReleaseEnvelopes, WRITER_APP, DICTATE_APP } from './update-metadata.mjs';

test('release sidecars bind Windows and Mac artifacts without changing legacy signatures', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-update-test-'));
  try {
    const key = signer(), bytes = Buffer.from('signed artifact');
    for (const platform of ['windows-x86_64', 'darwin-aarch64']) {
      const artifact = join(root, platform);
      await writeFile(artifact, bytes);
      const signature = key.sign(bytes);
      await writeFile(`${artifact}.sig`, signature);
      const sidecar = await writeReleaseMetadata({ artifact, version:'0.21.0', platform,
        publicKey:key.publicKey, now:1800000000,
        sign: async path => writeFile(`${path}.sig`, key.sign(await readFile(path))) });
      const envelope = JSON.parse(await readFile(sidecar));
      assert.equal(verifyMetadata(envelope, bytes, '0.21.0', platform, key.publicKey, 1800000000).size, bytes.length);
      assert.equal(await readFile(`${artifact}.sig`, 'utf8'), signature);
      for (const [content, version, target, now] of [
        [bytes, '9.0.0', platform, 1800000000], [Buffer.from('tampered'), '0.21.0', platform, 1800000000],
        [bytes, '0.21.0', 'linux-x86_64', 1800000000], [bytes, '0.21.0', platform, 1800000000+2592000],
      ]) assert.throws(() => verifyMetadata(envelope, content, version, target, key.publicKey, now));
      assert.throws(() => verifyMetadata(envelope, bytes, '0.21.0', platform, signer().publicKey, 1800000000));
    }
    assert.ok(!(await readdir(root)).some(name => name.includes('writer-manifest')));
  } finally { await rm(root, {recursive:true, force:true}); }
});
test('signer failure leaves no apparently successful metadata and removes temporary payload', async () => {
  const root = await mkdtemp(join(tmpdir(), 'writer-update-test-'));
  try {
    const artifact=join(root,'setup.exe'), key=signer(), bytes=Buffer.from('MZ fixture');
    await writeFile(artifact, bytes); await writeFile(`${artifact}.sig`,key.sign(bytes));
    await assert.rejects(writeReleaseMetadata({artifact,version:'0.21.0',platform:'windows-x86_64',publicKey:key.publicKey,
      sign:async () => { throw Error('interrupted'); }}), /interrupted/);
    assert.deepEqual((await readdir(root)).sort(),['setup.exe','setup.exe.sig']);
  } finally { await rm(root,{recursive:true,force:true}); }
});
test('metadata requires stable semantic versions and bounded nonempty artifacts', () => {
  for (const version of ['0.21.0-beta', '0.21.0+build', '01.2.3', 'v1.0.0']) assert.throws(()=>manifestFor(Buffer.from('a'),version,'windows-x86_64'));
  assert.throws(()=>manifestFor(Buffer.alloc(0),'0.21.0','windows-x86_64'));
  assert.throws(()=>manifestFor({length:300*1024*1024+1},'0.21.0','windows-x86_64'));
});

test('publisher verification accepts a shorter signer-selected lifetime', () => {
  const key=signer(), bytes=Buffer.from('fixture');
  const manifest=manifestFor(bytes,'0.21.0','windows-x86_64',1800000000);
  manifest.expires_at=manifest.issued_at+60;
  const payload=Buffer.from(JSON.stringify(manifest));
  const envelope={payload:payload.toString('base64'),signature:key.sign(payload)};
  verifyMetadata(envelope,bytes,'0.21.0','windows-x86_64',key.publicKey,1800000030);
  assert.throws(()=>verifyMetadata(envelope,bytes,'0.21.0','windows-x86_64',key.publicKey,1800000060));
});

test('offline renewal requires the clean retained receipt and signatures and never rewrites original files', async () => {
  const { renewReleaseMetadata } = await import('./update-metadata.mjs');
  const { createHash } = await import('node:crypto');
  const root=await mkdtemp(join(tmpdir(),'writer-renew-test-'));
  try {
    const key=signer(), bytes=Buffer.from('MZ retained artifact'), artifact=join(root,'setup.exe');
    const signature=key.sign(bytes), hash=data=>createHash('sha256').update(data).digest('hex');
    await writeFile(artifact,bytes); await writeFile(`${artifact}.sig`,signature);
    const sign=async path=>writeFile(`${path}.sig`,key.sign(await readFile(path)));
    const originalPath=await writeReleaseMetadata({artifact,version:'0.21.0',platform:'windows-x86_64',publicKey:key.publicKey,sign,now:1800000000});
    const originalBytes=await readFile(originalPath), original=JSON.parse(originalBytes);
    const receipt={appVersion:'0.21.0',commit:'a'.repeat(40),dirty:false,sourceChanged:false,target:'x86_64-pc-windows-msvc',artifact:{bytes:bytes.length,sha256:hash(bytes)},signature:{bytes:Buffer.byteLength(signature),sha256:hash(signature)}};
    const options={artifact,receipt,original,output:join(root,'renewed.writer-release.json'),publicKey:key.publicKey,sign,now:1802592010};
    await renewReleaseMetadata(options);
    const renewed=JSON.parse(await readFile(options.output));
    assert.equal(verifyMetadata(renewed,bytes,'0.21.0','windows-x86_64',key.publicKey,options.now).issued_at,options.now);
    assert.deepEqual(await readFile(originalPath),originalBytes);
    assert.deepEqual(await readFile(artifact),bytes);
    assert.equal(await readFile(`${artifact}.sig`,'utf8'),signature);
    for(const receipt of [{...options.receipt,dirty:true},{...options.receipt,sourceChanged:true},{...options.receipt,appVersion:'9.0.0'},{...options.receipt,artifact:{...options.receipt.artifact,sha256:'b'.repeat(64)}},{...options.receipt,signature:{}}]) await assert.rejects(renewReleaseMetadata({...options,receipt}));
    await assert.rejects(renewReleaseMetadata({...options,now:1800000000}),/advance/);
    await assert.rejects(renewReleaseMetadata({...options,output:originalPath}),/EEXIST/);
    await assert.rejects(renewReleaseMetadata({...options,output:join(root,'failed'),sign:async()=>{throw Error('interrupted');}}),/interrupted/);
    assert.ok(!(await readdir(root)).some(name=>name.includes('writer-manifest') || name==='failed'));
  } finally { await rm(root,{recursive:true,force:true}); }
});

test('Node parser matches Rust envelope/base64/duplicate-field rejection without canonicalizing payloads', () => {
  const key=signer(),bytes=Buffer.from('fixture'),manifest=manifestFor(bytes,'0.21.0','windows-x86_64',1800000000);
  const envelope=json=>{const payload=Buffer.from(json);return {payload:payload.toString('base64'),signature:key.sign(payload)};};
  const valid=envelope(JSON.stringify(manifest,null,2));
  verifyMetadata(valid,bytes,'0.21.0','windows-x86_64',key.publicKey,1800000000);
  for(const invalid of [{...valid,extra:true},{...valid,payload:valid.payload+'!'},envelope(JSON.stringify(manifest).replace('{','{"schema":1,'))]) assert.throws(()=>verifyMetadata(invalid,bytes,'0.21.0','windows-x86_64',key.publicKey,1800000000));
});


test('initial legacy binding requires retained clean artifacts and does not rewrite them', async () => {
  const {bindLegacyMetadata}=await import('./update-metadata.mjs');
  const {createHash}=await import('node:crypto');
  const root=await mkdtemp(join(tmpdir(),'writer-bind-test-'));
  try {
    const key=signer(),bytes=Buffer.from('MZ retained legacy'),artifact=join(root,'setup.exe'),signature=key.sign(bytes);
    const hash=data=>createHash('sha256').update(data).digest('hex');
    await writeFile(artifact,bytes); await writeFile(`${artifact}.sig`,signature);
    const receipt={appVersion:'0.20.0',commit:'a'.repeat(40),dirty:false,sourceChanged:false,target:'x86_64-pc-windows-msvc',artifact:{bytes:bytes.length,sha256:hash(bytes)},signature:{bytes:Buffer.byteLength(signature),sha256:hash(signature)}};
    const options={artifact,receipt,output:join(root,'binding.json'),publicKey:key.publicKey,now:1800000000,sign:async path=>writeFile(`${path}.sig`,key.sign(await readFile(path)))};
    await bindLegacyMetadata(options);
    const envelope=JSON.parse(await readFile(options.output));
    assert.equal(verifyMetadata(envelope,bytes,'0.20.0','windows-x86_64',key.publicKey,options.now).sha256,hash(bytes));
    assert.deepEqual(await readFile(artifact),bytes);
    assert.equal(await readFile(`${artifact}.sig`,'utf8'),signature);
    for (const changed of [{dirty:true},{sourceChanged:true},{artifact:{}},{signature:{}},{target:'unknown'}]) await assert.rejects(bindLegacyMetadata({...options,receipt:{...receipt,...changed}}));
    await assert.rejects(bindLegacyMetadata({...options,publicKey:signer().publicKey}));
  } finally {await rm(root,{recursive:true,force:true});}
});

test('each release carries a Metrix Writer and a TrickTic Dictate envelope for the same artifact', async () => {
  const {renewReleaseMetadata}=await import('./update-metadata.mjs');
  const {createHash}=await import('node:crypto');
  const root=await mkdtemp(join(tmpdir(),'writer-dual-test-'));
  try {
    const key=signer(),bytes=Buffer.from('MZ dual identity'),artifact=join(root,'setup.exe'),signature=key.sign(bytes);
    await writeFile(artifact,bytes); await writeFile(`${artifact}.sig`,signature);
    const sign=async path=>writeFile(`${path}.sig`,key.sign(await readFile(path)));
    const sidecars=await writeReleaseEnvelopes({artifact,version:'0.40.0',platform:'windows-x86_64',publicKey:key.publicKey,sign,now:1800000000});
    assert.deepEqual(sidecars,{writerRelease:`${artifact}.writer-release.json`,dictateRelease:`${artifact}.dictate-release.json`});
    const writer=JSON.parse(await readFile(sidecars.writerRelease)),dictate=JSON.parse(await readFile(sidecars.dictateRelease));
    const old=verifyMetadata(writer,bytes,'0.40.0','windows-x86_64',key.publicKey,1800000000,WRITER_APP);
    const current=verifyMetadata(dictate,bytes,'0.40.0','windows-x86_64',key.publicKey,1800000000,DICTATE_APP);
    assert.deepEqual([old.app,current.app],[WRITER_APP,DICTATE_APP]);
    assert.deepEqual({...old,app:null},{...current,app:null});
    // Neither envelope passes for the other identity.
    assert.throws(()=>verifyMetadata(writer,bytes,'0.40.0','windows-x86_64',key.publicKey,1800000000,DICTATE_APP));
    assert.throws(()=>verifyMetadata(dictate,bytes,'0.40.0','windows-x86_64',key.publicKey,1800000000,WRITER_APP));
    assert.throws(()=>manifestFor(bytes,'0.40.0','windows-x86_64',1800000000,'com.other.app'));
    // Renewal keeps each envelope's own app.
    const hash=data=>createHash('sha256').update(data).digest('hex');
    const receipt={appVersion:'0.40.0',commit:'a'.repeat(40),dirty:false,sourceChanged:false,target:'x86_64-pc-windows-msvc',artifact:{bytes:bytes.length,sha256:hash(bytes)},signature:{bytes:Buffer.byteLength(signature),sha256:hash(signature)}};
    for (const [original,app] of [[writer,WRITER_APP],[dictate,DICTATE_APP]]) {
      const output=join(root,`renewed-${app}.json`);
      await renewReleaseMetadata({artifact,receipt,original,output,publicKey:key.publicKey,sign,now:1802592010});
      assert.equal(verifyMetadata(JSON.parse(await readFile(output)),bytes,'0.40.0','windows-x86_64',key.publicKey,1802592010,app).app,app);
    }
  } finally {await rm(root,{recursive:true,force:true});}
});
