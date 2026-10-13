import { run, exitWith } from './process.mjs';
exitWith(run('cargo', ['run', '--locked', '-p', 'metrix-writer-api']));
