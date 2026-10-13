import { run, exitWith } from './process.mjs';
exitWith(run(process.platform === 'win32' ? 'target/release/metrix-writer-api.exe' : './target/release/metrix-writer-api', []));
