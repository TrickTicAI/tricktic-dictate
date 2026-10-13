import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const root = fileURLToPath(new URL('../../', import.meta.url));
export function run(command, args, options = {}) {
  // Only fixed repository commands use a Windows command shell.
  return spawn(command, args, { cwd: root, stdio: 'inherit', shell: process.platform === 'win32', ...options });
}
export function exitWith(child) {
  child.on('error', (error) => { console.error(error.message); process.exitCode = 1; });
  child.on('exit', (code) => { process.exitCode = code ?? 1; });
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
}
