import { verifySpeechProtocol } from './speech-runtime.mjs';
// Read-only preflight shared with the Darwin installer integration.
import { readFile } from 'node:fs/promises';
import { verifyRuntime } from './runtime.mjs';
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--resources') throw Error('Usage: node scripts/desktop/check-macos-runtime.mjs --resources <approved package>');
const lock = JSON.parse(await readFile(new URL('./macos-runtime.lock.json', import.meta.url)));
await verifyRuntime(args[1], lock);
await verifySpeechProtocol(args[1], lock, new URL('../../runtime/whisper/writer-auth.h', import.meta.url));
console.log(`macOS runtime preflight passed: ${lock.files.length} pinned runtime/notice files verified.`);
