import type { ModeProfile } from './api';
import { PRODUCT_NAME } from '../../product';

/** Shown at context opt-in and before processing, including default Command. */
export function ContextNotice({ profile }: { profile: ModeProfile }) {
  if (profile.kind === 'regular') return null;
  const provider = profile.text_model?.provider;
  const destination = provider === 'local'
    ? 'your configured local text server'
    : provider === 'openrouter'
      ? 'OpenRouter and the provider serving your selected model'
      : provider ? `${provider} cloud text processing` : 'the text provider you select';
  return <p className="small muted" data-testid="context-consent">
    Enabling context allows {PRODUCT_NAME} to save selected text, application or window titles, and current clipboard text before the microphone starts, and send the enabled fields with your transcript to {destination}.
    {profile.fallback_model && <> The configured backup also receives the same enabled fields through {profile.fallback_model.provider} if it runs.</>}
    {' '}Processing while speaking can send them before you stop. Context is removed after successful processing; failed or interrupted attempts keep it for retry until you delete all history. Raw transcripts and results stay separate and may contain text from that context.
    {' '}Turning context off affects future captures. Delete audio removes only audio; use Settings → Data &amp; privacy → Delete all audio and transcripts to remove retained context too.
  </p>;
}
