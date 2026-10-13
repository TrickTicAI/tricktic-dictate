import { isMac } from './platform';
import { useEffect, useState } from 'react';
import { invoke } from '../desktop-session/runtime/native';
import { preferencesBridge } from '../desktop-preferences/bridge';
import { Group, Row } from './controls';
import { PRODUCT_NAME } from '../../product';
import { offersSignIn, useAccount, useAccountTasks } from '../account/account';
import { GoogleAccount } from '../account/AccountPanel';
import { HistorySync } from '../account/HistorySync';
import { BackupSync } from '../account/BackupSync';
import { usePortable } from '../account/portable';
import { AccountData } from '../account/AccountData';
import { Cloud, HardDrive, UserRound } from 'lucide-react';
import { StorageControls } from './StorageControls';

export function DataPrivacy({ integrated, recordings }: {
  integrated: boolean;
  /** The saved recordings, for how much is kept here. */
  recordings?: { bytes: number }[];
}) {
  const [root, setRoot] = useState('');
  const [status, setStatus] = useState('');
  // Deleting audio and transcripts from every dictation, chosen in Free up
  // space, which has already checked that nothing is recording or processing.
  async function deleteHistory() {
    try {
      await invoke<{ deleted_recordings: number; deleted_jobs: number }>('history_delete');
      // The native store and this origin's cached requests are separate stores.
      // Clear the cache only after the shell accepts deletion while idle.
      await clearAttemptCache();
      window.location.reload();
    } catch (cause) {
      // Native capture stays paused through cache clearing and the normal reload.
      // A failed deletion/cache write must release that pause so recording can recover.
      await invoke('history_resume').catch(() => {});
      throw typeof cause === 'string' || cause instanceof Error ? cause : 'History could not be deleted. Stop recording and wait for transcription to finish, then retry.';
    }
  }

  useEffect(() => {
    if (!integrated) return;
    void invoke<{ root: string }>('data_location')
      .then((value) => setRoot(value.root))
      .catch(() => setStatus('The storage location could not be read.'));
  }, [integrated]);

  const here = integrated ? (isMac() ? 'On this Mac' : 'On this PC') : 'On your server';
  const kept = recordings ?? [];
  return (
    <>
      {/* The question this page answers first: where does my data go? */}
      <ul className="data-cards" aria-label="Where your data goes">
        <li className="data-card">
          <span className="data-card-icon" aria-hidden><HardDrive size={18} /></span>
          <strong>{here}</strong>
          <p>Recordings, transcripts, history and models.{kept.length ? ` ${kept.length.toLocaleString()} dictation${kept.length === 1 ? '' : 's'} kept.` : ''}{integrated ? ' What takes the space is under Storage.' : ''}</p>
        </li>
        <li className="data-card">
          <span className="data-card-icon" aria-hidden><Cloud size={18} /></span>
          <strong>Cloud providers</strong>
          <p>Only what you choose to send: audio for a cloud speech model, text for Super, Command and cleanup.</p>
        </li>
        <li className="data-card">
          <span className="data-card-icon" aria-hidden><UserRound size={18} /></span>
          <strong>Your account</strong>
          <p>Only if you sign in: your dictionary and settings, and transcript history if you turn it on.</p>
        </li>
      </ul>

      <Group title="Where your recordings live">
        <Row
          title={integrated ? (isMac() ? 'On this Mac' : 'On this PC') : 'On your server and in this browser'}
          detail={integrated
            ? `Audio, transcripts, job history and model weights are kept in a folder owned by your ${isMac() ? 'macOS' : 'Windows'} account, outside the installation directory. Audio and text are not encrypted by ${PRODUCT_NAME}. Uninstalling or repairing ${PRODUCT_NAME} does not remove this folder.`
            : `Audio stays in this browser after upload, with another copy and transcripts on your self-hosted server. ${PRODUCT_NAME} does not encrypt these stored copies. Clearing browser site data does not delete server data.`}
          stacked
        >
          {integrated && (
            <div className="row">
              <code className="small faint" style={{ overflowWrap: 'anywhere' }}>{root || 'Reading…'}</code>
              <button
                type="button"
                className="button"
                onClick={() => preferencesBridge.openDataFolder().catch(() => setStatus('The folder could not be opened.'))}
              >
                Open folder
              </button>
            </div>
          )}
        </Row>

        <Row
          title="Provider keys"
          detail={integrated
            ? `${isMac() ? 'Stored in your macOS login Keychain' : 'Stored by Windows under DPAPI for your account'}. They are never sent to this window and there is no HTTP route that could write one.`
            : 'Read from your server’s environment. They are never sent to this browser.'}
        />
      </Group>

      {integrated ? <StorageControls onDeleteEverything={deleteHistory} /> : (
        <Group title="Deleting data">
          <Row
            title="Delete all audio and transcripts"
            detail="Bulk deletion is available in the desktop app. Browser and server copies are separate; clearing browser site data does not delete server recordings."
          />
        </Group>
      )}

      <Group title="Where processing happens">
        <Row
          title="Speech and text models are separate choices"
          detail={integrated
            ? 'Local speech models process audio on this computer. Cloud speech models send audio to the selected provider. Cloud cleanup, Super or Command sends text to its selected provider even when speech recognition is local.'
            : 'Local speech recognition runs on your server, not in this browser. Cloud speech models receive uploaded audio; optional cloud text processing sends transcript text onward.'}
        />
        <Row
          title="Processing while you speak"
          detail="Super can send provisional text and enabled context to its text model before you stop. The factory Super profile enables Process while speaking; review this in Modes. Canceling cannot recall a request already sent."
        />
        {integrated && (
          <Row
            title="Optional application, selected-text and clipboard context"
            detail="Review context in Modes before recording. Enabled context can be sent to the text model and saved on disk with processing history. Turning it off affects future recordings; bulk history deletion removes saved snapshots. Deleting only audio does not erase transcript or context history."
          />
        )}
      </Group>

      <Group title={`Copies outside ${PRODUCT_NAME}`}>
        {integrated && <Row title="Optional account sync" detail="Audio stays on this device. When enabled in Account, transcript text, settings and dictionary entries sync separately to your account in the Writer Firebase project’s Cloud Firestore database (me-west1). Text is readable by the service; it is not end-to-end encrypted. Local storage deletion does not erase cloud copies. Use Account → Delete cloud data for that." />}
        <Row
          title="Clipboard, exports and backups"
          detail="Copied, pasted or exported text and audio can remain with the receiving app, clipboard history, sync services or backups. Preserving the clipboard restores prior contents; it does not erase those copies."
        />
        <Row
          title="Cloud providers and updates"
          detail="Local deletion does not erase provider-held data or cancel requests already sent. Provider retention and billing depend on your account. Model downloads and update checks can use the network even when speech recognition is local."
        />
      </Group>

      {integrated && !isMac() && (
        <Group title="Earlier versions">
          {/* Copy kept verbatim from the retired native menu item. */}
          <Row
            title="Previous localhost recordings"
            detail={`Previous recordings remain in the old WebView2 profile and origin. Quit ${PRODUCT_NAME}, then use the Start Menu entry ‘Metrix Writer — previous localhost recordings’. Start your previous server to replay/download its audio and text. You can also use --url with your previous HTTPS origin. Nothing is moved or cleared. See README.txt for backup and recovery.`}
          />
        </Group>
      )}

      <Group title="Processing context">
        <Row title="Selected text, application and clipboard context" detail="Enabled mode context is saved before the microphone starts and sent to the selected text provider, including during speech if enabled. Successful processing removes its context inputs. Failed or interrupted attempts keep them for retry; turning context off only affects future captures. Raw transcripts and results are retained independently and may contain context text. Local models use your configured local server; cloud models receive enabled context, and OpenRouter also forwards it to the model provider." />
        <Row title="Delete audio or delete history" detail="Deleting audio removes the recordings only. It keeps transcripts, results and any context still needed for retry. Deleting audio and transcripts also removes saved mode context and processing history. Neither erases copies already sent to providers, exported files, backups, or your operating system’s clipboard history." />
      </Group>


      <Group title="Security reports">
        <Row
          title="Report a suspected vulnerability privately"
          detail={`Email report@tricktic.com with your ${PRODUCT_NAME} version, platform and steps using invented text or synthetic audio. Do not include provider keys, private recordings or transcripts.`}
        />
      </Group>

      {status && <p className="notice alert" role="alert">{status}</p>}
    </>
  );
}

export function Account({ integrated }: { integrated: boolean }) {
  // Public builds report sign-in as unavailable and show only the first group.
  const account = useAccount(integrated);
  const portable = usePortable(integrated);
  const signedIn = account.snapshot?.state === 'signed_in';
  const tasks = useAccountTasks(integrated && offersSignIn(account.snapshot));
  return (
    <>
      <Group title="Account">
        <Row
          title="No account needed"
          detail={integrated
            ? `${PRODUCT_NAME} needs no account. Local models run on this ${isMac() ? 'Mac' : 'PC'}; choosing a cloud model sends audio to that provider. Audio stays on this device. Transcript history sync is a separate choice below.`
            : `${PRODUCT_NAME} uses your configured server; the server operator controls access and can read uploaded audio and transcripts. Recordings are not automatically synchronised between devices.`}
        />
      </Group>
      {integrated && offersSignIn(account.snapshot) && <GoogleAccount view={account} />}
      {integrated && <HistorySync key={String(account.snapshot?.generation)} signedIn={signedIn} available={portable.status?.sync_available ?? false} />}
      {integrated && <BackupSync status={portable.status} signedIn={signedIn} onChanged={portable.reload} />}
      {integrated && offersSignIn(account.snapshot) && <AccountData view={account} tasks={tasks} />}
    </>
  );
}

async function clearAttemptCache(): Promise<void> {
  const database = await new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('metrix-writer-transcription', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('attempts', { keyPath: 'id' });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(new Error('Saved audio was deleted, but the transcript cache could not be opened. Retry to clear it.'));
  });
  try {
    await new Promise<void>((resolve, reject) => {
      const tx = database.transaction('attempts', 'readwrite');
      tx.objectStore('attempts').clear();
      tx.oncomplete = () => resolve();
      tx.onabort = tx.onerror = () => reject(new Error('Saved audio was deleted, but the transcript cache could not be cleared. Retry to clear it.'));
    });
  } finally { database.close(); }
}
