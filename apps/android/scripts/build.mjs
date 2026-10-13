// Compatibility entrypoint; Gradle owns Kotlin/Java compilation, packaging and signing.
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const result = spawnSync(process.platform === 'win32' ? 'gradlew.bat' : './gradlew', [':app:assembleDevDebug', ...process.argv.slice(2)], {
  cwd: root, stdio: 'inherit', windowsHide: true, shell: process.platform === 'win32',
});
if (result.error) throw result.error;
process.exit(result.status ?? 1);
