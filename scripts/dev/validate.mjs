import { run } from './process.mjs';

// Deterministic security fixtures run offline; advisory/release gates are explicit.
for (const [command, ...args] of [
  ['python3', '-m', 'unittest', 'discover', '-s', 'scripts/security', '-p', 'test_*.py'],
  ['cargo', 'fmt', '--all', '--check'],
  ['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings'],
  ['cargo', 'test', '--workspace', '--locked'],
  ['pnpm', '--filter', '@metrix-writer/web', 'build'],
]) {
  console.log(`\n> ${[command, ...args].join(' ')}`);
  const child = run(command, args);
  const code = await new Promise((resolve) => {
    child.on('error', (error) => { console.error(error.message); resolve(1); });
    child.on('exit', resolve);
  });
  if (code !== 0) process.exit(code ?? 1);
}
