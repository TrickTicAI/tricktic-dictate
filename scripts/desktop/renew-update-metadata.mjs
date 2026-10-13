// Offline command: no network or publisher credentials; signer output is captured.
import { readFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { renewReleaseMetadata } from './update-metadata.mjs';
const repo = fileURLToPath(new URL('../../',import.meta.url));
try {
  const args=process.argv.slice(2), opts={};
  for(let i=0;i<args.length;i+=2) {
    if (!['--artifact','--receipt','--original','--output'].includes(args[i]) || !args[i+1] || args[i+1].startsWith('--') || opts[args[i]]) throw Error('Invalid arguments');
    opts[args[i]]=resolve(args[i+1]);
  }
  if(Object.keys(opts).length!==4 || !process.env.WRITER_SIGNING_KEY_FILE) throw Error('Use --artifact file --receipt file --original sidecar --output new-sidecar with WRITER_SIGNING_KEY_FILE');
  const receipt=JSON.parse(await readFile(opts['--receipt'],'utf8'));
  if (!/^[a-f0-9]{40}$/.test(receipt.commit ?? '')) throw Error('Invalid receipt commit');
  const config=JSON.parse(await readFile(new URL('../../apps/desktop/tauri/tauri.conf.json',import.meta.url)));
  const source=JSON.parse(execFileSync('git',['-C',repo,'show',`${receipt.commit}:apps/desktop/tauri/tauri.conf.json`],{encoding:'utf8',stdio:['ignore','pipe','pipe']}));
  if(source.version!==receipt.appVersion || source.plugins.updater.pubkey!==config.plugins.updater.pubkey) throw Error('Retained source version/signing authority does not match receipt');
  const env={...process.env,TAURI_SIGNING_PRIVATE_KEY_PATH:resolve(process.env.WRITER_SIGNING_KEY_FILE)};
  delete env.TAURI_SIGNING_PRIVATE_KEY;
  await renewReleaseMetadata({artifact:opts['--artifact'],receipt,original:JSON.parse(await readFile(opts['--original'],'utf8')),output:opts['--output'],publicKey:config.plugins.updater.pubkey,
    sign:async path=>{execFileSync(process.execPath,[resolve(repo,'node_modules/@tauri-apps/cli/tauri.js'),'signer','sign',path],{env,stdio:['ignore','pipe','pipe']});}});
  console.log(`Signed renewal: ${opts['--output']}. Retained artifact and original sidecar unchanged.`);
} catch {
  // Do not expose subprocess output, key contents, or credential-bearing errors.
  console.error('Offline metadata renewal failed. Check arguments, retained clean receipt/source, original sidecar, artifact signature and protected signing key.');
  process.exitCode=1;
}
