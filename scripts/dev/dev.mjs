import { run } from './process.mjs';

// Vite preserves the browser Host when forwarding /api.
process.env.WRITER_HTTP_ORIGIN ??= `http://${process.env.WRITER_WEB_HOST ?? '127.0.0.1'}:${process.env.WRITER_WEB_PORT ?? 5174}`;

const children = [
  run('cargo', ['run', '--locked', '-p', 'metrix-writer-api']),
  run('pnpm', ['--filter', '@metrix-writer/web', 'dev']),
];
let stopping = false;
function stop(code) {
  if (stopping) return;
  stopping = true;
  process.exitCode = code;
  for (const child of children) child.kill('SIGTERM');
}
for (const child of children) {
  child.on('error', (error) => { console.error(error.message); stop(1); });
  child.on('exit', (code) => stop(code ?? 1));
}
process.on('SIGINT', () => stop(0));
process.on('SIGTERM', () => stop(0));
