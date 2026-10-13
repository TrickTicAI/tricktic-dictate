import { spawn } from 'node:child_process';
import { join } from 'node:path';
const sdk = process.env.WRITER_ANDROID_SDK || join(process.env.LOCALAPPDATA,'Android/Sdk');
const child=spawn(join(sdk,'platform-tools/adb.exe'),['-s','emulator-5580',...process.argv.slice(2)],{windowsHide:true,stdio:['pipe','pipe','pipe']});
process.stdin.pipe(child.stdin);child.stdout.pipe(process.stdout);child.stderr.pipe(process.stderr);child.on('exit',code=>process.exitCode=code);
