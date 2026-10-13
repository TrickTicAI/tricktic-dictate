// Dedicated Writer emulator only. No window, no host audio, no shared adb restart.
import { spawn } from 'node:child_process';
import { mkdir, writeFile, open } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
if (process.platform !== 'win32') throw Error('Use native Windows Node');
const sdk = process.env.WRITER_ANDROID_SDK || join(process.env.LOCALAPPDATA,'Android/Sdk');
const java = process.env.WRITER_ANDROID_JAVA || 'C:/Program Files/Eclipse Adoptium/jdk-17.0.18.8-hotspot';
const data = process.env.WRITER_ANDROID_EMULATOR || join(process.env.LOCALAPPDATA,'metrix-writer/android-emulator-'+Date.now());
const avdName = 'Writer_MW14'; const port = 5580; const serial = `emulator-${port}`;
const env = { ...process.env, JAVA_HOME: java, ANDROID_AVD_HOME: join(data,'avds'), PATH: `${join(java,'bin')};${process.env.PATH}` };
async function run(exe,args,input='') {
 return await new Promise((resolve,reject) => { const child=spawn(exe,args,{env,windowsHide:true,stdio:['pipe','pipe','pipe']}); let out=''; child.stdout.on('data',b=>out+=b); child.stderr.on('data',b=>out+=b); child.on('error',reject); child.on('exit',code=>code===0?resolve(out):reject(Error(out))); child.stdin.end(input); });
}
await mkdir(env.ANDROID_AVD_HOME,{recursive:true});
const adb = join(sdk,'platform-tools/adb.exe');
const devices = await run(adb,['devices']); if (devices.includes(serial)) throw Error('Writer emulator port already in use; inspect ownership');
if (existsSync(join(data,'receipt.json'))) throw Error('Use a fresh private WRITER_ANDROID_EMULATOR directory');
await run(join(java,'bin/java.exe'),['-Dcom.android.sdkmanager.toolsdir='+join(sdk,'cmdline-tools/latest'),'-classpath',join(sdk,'cmdline-tools/latest/lib/avdmanager-classpath.jar'),'com.android.sdklib.tool.AvdManagerCli','create','avd','--name',avdName,'--package','system-images;android-36.1;google_apis;x86_64','--path',join(data,'device')],'no\n');
const stdout=await open(join(data,'emulator.stdout.log'),'w'); const stderr=await open(join(data,'emulator.stderr.log'),'w');
const child=spawn(join(sdk,'emulator/emulator.exe'),['-avd',avdName,'-port',String(port),'-no-window','-no-audio','-no-boot-anim','-no-snapshot','-gpu','swiftshader_indirect','-memory','2048','-cores','2','-no-metrics'],{env,windowsHide:true,detached:true,stdio:['ignore',stdout.fd,stderr.fd]});
child.unref(); await stdout.close(); await stderr.close();
await writeFile(join(data,'receipt.json'),JSON.stringify({pid:child.pid,serial,data,headless:true,hostAudio:false},null,2));
for(let i=0;i<150;i++) { try { if((await run(adb,['-s',serial,'shell','getprop','sys.boot_completed'])).trim()==='1') { console.log(JSON.stringify({ready:true,serial,data,pid:child.pid})); process.exit(0); } } catch {} await new Promise(r=>setTimeout(r,2000)); }
throw Error('Emulator boot timed out; inspect private logs and stop only recorded emulator');
