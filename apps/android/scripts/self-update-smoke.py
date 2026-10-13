#!/usr/bin/env python3
"""Prove in-app release self-update via PackageInstaller on an owned emulator (never a phone).

Without --feed-url the candidate APK is staged locally and committed through the
updater, proving installation and data retention. With --feed-url the installed
bootstrap detects, downloads and verifies the candidate from the hosted channel
through production updater code before committing it, proving the full feed path.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

import dev
import release

PACKAGE = 'com.metrix.writer'
COMPONENT = PACKAGE + '/com.metrix.writer.WriterInputService'


def find_button_bounds(xml_text, button_text):
    """Find bounds [x,y] center of a button with given text in uiautomator dump XML."""
    match = re.search(r'text="' + re.escape(button_text) + r'"[^>]*bounds="\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]"', xml_text)
    if match:
        x1, y1, x2, y2 = map(int, match.groups())
        return ((x1 + x2) // 2, (y1 + y2) // 2)
    return None


def main():
    os.umask(0o077)
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--serial', required=True)
    p.add_argument('--previous', type=Path, required=True)
    p.add_argument('--current', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--feed-url', default=None,
                   help='Hosted channel manifest for full detect/download/verify proof')
    p.add_argument('--probe-refusal', action='store_true',
                   help='Revoked install permission must fail the update cleanly, then re-grant')
    p.add_argument('--probe-cancel-download', action='store_true',
                   help='Cancel a download mid-flight; state must settle to Available or Downloaded')
    p.add_argument('--probe-restart', action='store_true',
                   help='Reboot between download and install; the update must resume and complete')
    p.add_argument('--cancel-at-prompt', action='store_true',
                   help='Dismiss the installer prompt; nothing must install and retry must stay available')
    args = p.parse_args()
    hosted = args.feed_url is not None
    if args.probe_cancel_download and not hosted:
        raise ValueError('--probe-cancel-download needs --feed-url')
    if args.probe_restart and not hosted:
        raise ValueError('--probe-restart needs --feed-url')

    if not args.serial.startswith('emulator-'):
        raise ValueError('Self-update fixtures must never run on a physical phone')

    dev.selected(args.serial)
    owner = dev.owned(args.serial)
    output = release.external(args.output)
    output.mkdir(parents=True, exist_ok=False)

    before = release.inspect(args.previous / 'writer.apk')
    after = release.inspect(args.current / 'writer.apk')

    if before['signer_sha256'] != after['signer_sha256']:
        raise ValueError(f"Signer mismatch: {before['signer_sha256']} != {after['signer_sha256']}")
    if before['version_code'] >= after['version_code']:
        raise ValueError(f"Version code must increase: {before['version_code']} >= {after['version_code']}")

    def shell(*items):
        return dev.adb(args.serial, 'shell', *items).decode('utf-8', errors='replace').strip()

    # Clean previous installation
    if ('package:' + PACKAGE) in shell('pm', 'list', 'packages', PACKAGE).splitlines():
        dev.adb(args.serial, 'uninstall', PACKAGE)
    if ('package:' + PACKAGE + '.test') in shell('pm', 'list', 'packages', PACKAGE + '.test').splitlines():
        dev.adb(args.serial, 'uninstall', PACKAGE + '.test')

    services = shell('settings', 'get', 'secure', 'enabled_accessibility_services')
    services = '' if services == 'null' else services

    def install_bootstrap(directory):
        print(f"Installing bootstrap release {before['version_name']} (code {before['version_code']})...")
        for name in ('writer.apk', 'upgrade-test.apk'):
            dev.adb(args.serial, 'install', '-r', str(directory / name))

    def phase(name, version_code, timeout=120, **extra_args):
        command = [dev.tool('platform-tools/adb'), '-s', args.serial, 'shell', 'am', 'instrument', '-w',
                   '-e', 'phase', name, '-e', 'version', str(version_code)]
        for k, v in extra_args.items():
            command += ['-e', k, str(v)]
        command += [PACKAGE + '.test/com.metrix.writer.UpgradeRunner']
        result = subprocess.run(command, check=True, capture_output=True, timeout=timeout).stdout.decode()
        (output / f"{name}.txt").write_text(result)
        if 'result=PASS' not in result or 'INSTRUMENTATION_CODE: 0' not in result:
            raise RuntimeError(f"Phase {name} failed: {result}")
        return result

    def phase_value(name, key):
        text = (output / f"{name}.txt").read_text()
        match = re.search(r'INSTRUMENTATION_RESULT: ' + re.escape(key) + r'=(\S+)', text)
        return match.group(1) if match else None

    def wait_for_boot(timeout_s=240):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                if dev.adb(args.serial, 'shell', 'getprop', 'sys.boot_completed').strip() == b'1':
                    return
            except subprocess.CalledProcessError:
                pass
            time.sleep(3)
        raise TimeoutError('Emulator did not finish booting in time')

    def get_installed_version_code():
        line = shell('pm', 'list', 'packages', '--show-versioncode', PACKAGE)
        match = re.search(r'package:' + re.escape(PACKAGE) + r' versionCode:([0-9]+)', line)
        return int(match.group(1)) if match else None

    # Step 1: Install bootstrap version
    install_bootstrap(args.previous)
    assert get_installed_version_code() == before['version_code'], "Bootstrap version code mismatch"

    # Step 2: Grant permissions
    for permission in ('android.permission.RECORD_AUDIO', 'android.permission.POST_NOTIFICATIONS'):
        shell('pm', 'grant', PACKAGE, permission)
    shell('appops', 'set', PACKAGE, 'SYSTEM_ALERT_WINDOW', 'allow')
    shell('appops', 'set', PACKAGE, 'REQUEST_INSTALL_PACKAGES', 'allow')
    shell('settings', 'put', 'secure', 'enabled_accessibility_services', ':'.join(filter(None, [services, COMPONENT])))
    shell('settings', 'put', 'secure', 'accessibility_enabled', '1')

    shell('am', 'start', '-W', '-n', PACKAGE + '/com.metrix.writer.FloatingSetupActivity')
    time.sleep(2)

    # Step 3: Seed data (recordings, history, credentials, preferences)
    print("Seeding test data in bootstrap version...")
    phase('seed', before['version_code'])

    probes = {}
    staged_tmp = app_cache_apk = None
    if not hosted:
        # Step 4: Push candidate APK to staging location for in-app updater
        staged_tmp = '/data/local/tmp/update_candidate.apk'
        dev.adb(args.serial, 'push', str(args.current / 'writer.apk'), staged_tmp)
        app_cache_apk = f'/data/data/{PACKAGE}/cache/update_candidate.apk'
        shell('su', '0', 'cp', staged_tmp, app_cache_apk)
        owner_uid = shell('su', '0', 'stat', '-c', '%u:%g', f'/data/data/{PACKAGE}')
        shell('su', '0', 'chown', owner_uid, app_cache_apk)
        shell('su', '0', 'chmod', '644', app_cache_apk)

    if args.probe_refusal:
        print("Probe: revoked install permission must fail the update cleanly...")
        shell('appops', 'set', PACKAGE, 'REQUEST_INSTALL_PACKAGES', 'deny')
        try:
            if hosted:
                probe_name, probe_args = 'hosted-update', dict(feed_url=args.feed_url,
                                                               target_code=after['version_code'])
            else:
                probe_name, probe_args = 'in-app-update', dict(candidate_apk=app_cache_apk,
                                                               target_version=after['version_name'],
                                                               target_code=after['version_code'])
            try:
                phase(probe_name, before['version_code'], timeout=240, **probe_args)
            except RuntimeError as failure:
                if 'permission' not in str(failure).lower():
                    raise
                print(f"Refusal probe failed as required: {str(failure)[:160]}")
            else:
                raise RuntimeError('Update succeeded despite revoked install permission')
            assert get_installed_version_code() == before['version_code'], 'Version changed during refusal probe'
            (output / f"{probe_name}.txt").rename(output / f"{probe_name}-refusal.txt")
        finally:
            shell('appops', 'set', PACKAGE, 'REQUEST_INSTALL_PACKAGES', 'allow')
        probes['refusal'] = 'blocked with permission error; version unchanged'

    if args.probe_cancel_download:
        print("Probe: cancel a download mid-flight...")
        phase('download-cancel', before['version_code'], timeout=240, feed_url=args.feed_url)
        probes['cancel_download'] = phase_value('download-cancel', 'outcome')

    if args.probe_restart:
        print("Probe: reboot between download and install...")
        phase('download-only', before['version_code'], timeout=240,
              feed_url=args.feed_url, target_code=after['version_code'])
        dev.adb(args.serial, 'reboot')
        wait_for_boot()
        probes['restart'] = 'rebooted after download; update resumes below'

    if hosted:
        # Step 5 (hosted): the installed bootstrap detects, downloads and
        # verifies the candidate from the channel through production updater code.
        print(f"Triggering hosted in-app self-update from {args.feed_url}...")
        phase('hosted-update', before['version_code'], timeout=240,
              feed_url=args.feed_url,
              target_code=after['version_code'])
    else:
        # Step 5 (staged): Trigger in-app self-update via PackageInstaller through UpgradeRunner
        print(f"Triggering in-app self-update to {after['version_name']} (code {after['version_code']})...")
        phase('in-app-update', before['version_code'],
              candidate_apk=app_cache_apk,
              target_version=after['version_name'],
              target_code=after['version_code'])

    # Step 6: Handle confirmation prompt if presented by system, or verify promptless install
    prompt_observed = False
    cancelled = False
    start_time = time.monotonic()
    deadline = start_time + 30

    while time.monotonic() < deadline:
        current_code = get_installed_version_code()
        if current_code == after['version_code']:
            if args.cancel_at_prompt:
                raise RuntimeError('Update installed despite cancel-at-prompt mode')
            print(f"Self-update completed successfully! Installed version code: {current_code}")
            break

        # Check for system package installer dialog
        shell('uiautomator', 'dump', '/data/local/tmp/uidump.xml')
        dump = shell('cat', '/data/local/tmp/uidump.xml')

        # Look for "Update" button
        bounds = find_button_bounds(dump, 'Update') or find_button_bounds(dump, 'Install')
        if bounds:
            prompt_observed = True
            if args.cancel_at_prompt:
                print(f"Observed system installer prompt; dismissing with BACK at {bounds}...")
                shell('input', 'keyevent', 'KEYCODE_BACK')
                cancelled = True
                time.sleep(3)
                break
            print(f"Observed system installer prompt; tapping confirmation at {bounds}...")
            shell('input', 'tap', str(bounds[0]), str(bounds[1]))
            time.sleep(1)

        time.sleep(0.5)
    else:
        raise TimeoutError(f"Self-update timed out. Current version code: {get_installed_version_code()}")

    update_duration_s = time.monotonic() - start_time
    if args.cancel_at_prompt:
        if not cancelled:
            raise RuntimeError('No installer prompt appeared to cancel')
        assert get_installed_version_code() == before['version_code'], 'Version changed after prompt dismissal'
        phase('state', before['version_code'])
        update_state = phase_value('state', 'update_state')
        assert update_state == 'Downloaded', f'Expected retryable Downloaded state, got {update_state}'
        print('Prompt dismissal left the update retryable (Downloaded).')
        verify_code, expected_state = before['version_code'], 'Downloaded'
    else:
        assert get_installed_version_code() == after['version_code'], "Version code was not updated"
        verify_code, expected_state = after['version_code'], 'Idle'
        # Verify with the candidate's own same-signer runner, as upgrade-smoke.py does: the
        # bootstrap's runner is compiled against the older app classes.
        dev.adb(args.serial, 'install', '-r', str(args.current / 'upgrade-test.apk'))

    # Step 7: Launch app and verify idle capture
    shell('am', 'start', '-W', '-n', PACKAGE + '/com.metrix.writer.FloatingSetupActivity')
    time.sleep(2)

    activity_services = shell('dumpsys', 'activity', 'services', PACKAGE)
    if '.CaptureService' in activity_services:
        raise RuntimeError('Capture service started automatically after self-update')

    # Step 8: Run verify phase for data retention, credential decryption and updater state
    print("Verifying data retention, credential decryption, and state post-update...")
    verify_result = phase('verify', verify_code, expected_state=expected_state)

    # Clean up staging files
    cleanup = ['/data/local/tmp/uidump.xml']
    if staged_tmp is not None:
        cleanup += [staged_tmp, app_cache_apk]
    shell('su', '0', 'rm', '-f', *cleanup)

    receipt = {
        'passed': True,
        'serial': args.serial,
        'emulator_pid': owner['pid'],
        'method': ('Hosted channel detect/download/verify/commit via production updater'
                   if hosted else
                   'PackageInstaller in-app self-update (no adb install used for candidate)'),
        'feed_url': args.feed_url,
        'previous': before,
        'current': after,
        'probes': probes,
        'install_outcome': 'cancelled' if args.cancel_at_prompt else 'installed',
        'prompt_observed': prompt_observed,
        'update_duration_seconds': round(update_duration_s, 2),
        'verify_output': verify_result.strip(),
        'tested_at_utc': shell('date', '-u', '+%Y-%m-%dT%H:%M:%SZ')
    }

    (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps(receipt, indent=2))
    print("\nIn-app self-update acceptance PASSED.")


if __name__ == '__main__':
    main()
