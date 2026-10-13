import { useState } from 'react';
import { UpdatesPanel, type UpdateController } from '../shell/updates';
import { invoke } from '../desktop-session/runtime/native';
import { LEGAL_LINKS, PRODUCT_NAME, SUPPORT_EMAIL } from '../../product';
import { Group, Row } from './controls';
import { Licenses } from './Licenses';

type LegalLink = keyof typeof LEGAL_LINKS;

/**
 * Version, updates, and the legal pages and licenses a published app owes its
 * users.
 *
 * The page this replaced explained what Writer is, where inference runs, how
 * credentials are stored and how languages work. Those are true and they are
 * still stated — next to the settings that control them, where they change what
 * you do. Repeating them here made the application read as its own marketing
 * site, and told the user they were running a Windows desktop application while
 * they were looking at its window.
 */
export function About({ integrated, version, updates }: {
  integrated: boolean;
  version: string;
  updates?: UpdateController;
}) {
  const [licenses, setLicenses] = useState(false);
  const [status, setStatus] = useState('');

  if (licenses) return <Licenses onBack={() => setLicenses(false)} />;

  // The desktop window opens no other windows, so the shell hands a fixed
  // address to the default browser; the page only names which one.
  function link(kind: LegalLink, label: string) {
    if (!integrated) return <a className="button" href={LEGAL_LINKS[kind]} target="_blank" rel="noreferrer">{label}</a>;
    return (
      <button
        type="button"
        className="button"
        onClick={() => {
          setStatus('');
          invoke('legal_open', { kind }).catch((error: unknown) => setStatus(error instanceof Error ? error.message : String(error)));
        }}
      >
        {label}
      </button>
    );
  }

  return (
    <div className="about-panel">
      <div className="about-mark">
        <img src="/writer-icon.png" alt="" width={48} height={48} />
        <div>
          <h3>{PRODUCT_NAME}</h3>
          <p>Version {version}</p>
        </div>
      </div>

      {integrated && updates
        ? <UpdatesPanel updates={updates} />
        : (
          <p className="notice">
            This is the self-hosted browser client, so it updates when you deploy your server. In-app updates are
            part of the desktop app.
          </p>
        )}

      <div className="about-legal">
        <Group title="Legal and support" note={status || undefined} noteTone={status ? 'alert' : undefined}>
          <Row title="Privacy policy" detail={`What ${PRODUCT_NAME} keeps on this computer and what it sends to the providers you choose.`}>
            {link('privacy', 'Open')}
          </Row>
          <Row title="Terms of use">{link('terms', 'Open')}</Row>
          <Row
            title="Open-source licenses"
            detail={integrated
              ? 'Licenses and notices for the libraries, speech and text runtimes, and downloadable models. They are installed with the app and readable offline.'
              : 'The desktop app installs the license notices for everything it ships.'}
          >
            {integrated && <button type="button" className="button" onClick={() => setLicenses(true)}>View</button>}
          </Row>
          <Row title="Support" detail={SUPPORT_EMAIL}>{link('support', 'Email')}</Row>
          <Row title="Report a security issue" detail="How to report a suspected vulnerability privately.">
            {link('security', 'Open')}
          </Row>
        </Group>
      </div>
    </div>
  );
}
