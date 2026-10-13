#!/usr/bin/env python3
"""Explicit live-provider proof on an owned emulator, using a public/consented WAV and private key file."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import wave
import xml.etree.ElementTree as ET

import dev
import release


def main():
    os.umask(0o077)
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial',required=True)
    p.add_argument('--provider',choices=['gemini','openai','groq'],default='gemini')
    p.add_argument('--directory',type=Path,required=True)
    p.add_argument('--key-file',type=Path,required=True)
    p.add_argument('--wav',type=Path,required=True)
    p.add_argument('--consented',action='store_true',required=True)
    a=p.parse_args()
    if not a.serial.startswith('emulator-'):raise ValueError('Live test runner must never run on a phone')
    dev.selected(a.serial);owner=dev.owned(a.serial)
    output=release.external(a.directory);output.mkdir(parents=True,exist_ok=False)
    release.external(a.key_file.resolve().parent)
    if a.key_file.stat().st_mode & 0o077:raise ValueError('Provider key file must be mode 600')
    existing=dev.adb(a.serial,'shell','run-as',dev.PACKAGE,'ls','shared_prefs').decode()
    if 'direct-cloud.xml' in existing:
        prefs=ET.fromstring(dev.adb(a.serial,'exec-out','run-as',dev.PACKAGE,'cat','shared_prefs/direct-cloud.xml'))
        if any(n.get('name','').endswith('credential') for n in prefs):raise ValueError('Use an unconfigured emulator; refusing to replace its cloud key')
    backups = {}
    for name in ['direct-cloud.xml', 'companion.xml', 'speech.xml']:
        backups[name] = dev.adb(a.serial,'exec-out','run-as',dev.PACKAGE,'cat','shared_prefs/'+name) if name in existing else None
    canonical=output/'source.wav'
    with wave.open(str(a.wav),'rb') as source:
        if (source.getnchannels(),source.getsampwidth(),source.getframerate())!=(1,2,16000):raise ValueError('Provide PCM16 mono 16 kHz WAV')
        with wave.open(str(canonical),'wb') as dest:
            dest.setnchannels(1);dest.setsampwidth(2);dest.setframerate(16000)
            while data:=source.readframes(32768):dest.writeframesraw(data)
    env=dict(os.environ);env.pop('WRITER_ANDROID_TEST_CA',None)
    command=[str(dev.ROOT/'gradlew'),'--offline','-PwriterTestRunner=com.metrix.writer.DirectLiveRunner',':app:assembleDevDebug',':app:assembleDevDebugAndroidTest']
    with (output/'build.log').open('w') as log:subprocess.run(command,cwd=dev.ROOT,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    for f in ['app/build/outputs/apk/dev/debug/app-dev-debug.apk','app/build/outputs/apk/androidTest/dev/debug/app-dev-debug-androidTest.apk']:
        dev.adb(a.serial,'install','-r',str(dev.ROOT/f))
    base=[dev.tool('platform-tools/adb'),'-s',a.serial]
    def get(path):return dev.adb(a.serial,'exec-out','run-as',dev.PACKAGE,'cat',path)
    try:
        for source,name in [(a.key_file,'direct-live-key.private'),(canonical,'direct-live.wav')]:
            with source.open('rb') as data:
                subprocess.run(base+['exec-in','run-as',dev.PACKAGE,'sh','-c','cat > files/'+name],stdin=data,check=True,capture_output=True)
        with (output/'run.txt').open('w') as log:
            subprocess.run(base+['shell','am','instrument','-w','-e','provider',a.provider,dev.PACKAGE+'.test/com.metrix.writer.DirectLiveRunner'],stdout=log,stderr=subprocess.STDOUT,check=True,timeout=240)
        result=(output/'run.txt').read_text()
        if 'result=PASS' not in result or 'INSTRUMENTATION_CODE: 0' not in result:raise RuntimeError('Direct cloud run failed; see sanitized run.txt')
        receipt=json.loads(get('files/direct-live-receipt.json'))
        (output/'session.private.json').write_bytes(get('files/recordings/'+receipt['session']+'.json'))
        if receipt['audio_sha256']!=hashlib.sha256(canonical.read_bytes()).hexdigest():raise RuntimeError('Saved audio differs from canonical source')
        receipt.update(serial=a.serial,emulator_pid=owner['pid'],physical_microphone=False,
                       source_dirty=bool(release.run(["git","-C",dev.ROOT,"status","--porcelain"]).strip()),
                       source_commit=release.run(['git','-C',dev.ROOT,'rev-parse','HEAD']).strip(),
                       apk_sha256=hashlib.sha256((dev.ROOT/'app/build/outputs/apk/dev/debug/app-dev-debug.apk').read_bytes()).hexdigest())
        (output/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
        print(json.dumps(receipt,indent=2))
    finally:
        dev.adb(a.serial,'shell','am','force-stop',dev.PACKAGE)
        # This helper refused a previously configured account. Remove only its temporary key revisions/input.
        dev.adb(a.serial,'shell',f"run-as com.metrix.writer.debug sh -c 'rm -f no_backup/{a.provider}-*.credential files/direct-live-key.private'")
        for name, data in backups.items():
            if data is None:
                dev.adb(a.serial,'shell','run-as',dev.PACKAGE,'rm','-f','shared_prefs/'+name)
            else:
                subprocess.run(base+['exec-in','run-as',dev.PACKAGE,'sh','-c','cat > shared_prefs/'+name], input=data, check=True, capture_output=True)


if __name__=='__main__':main()
