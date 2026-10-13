import type { InputLanguage } from '@metrix-writer/contracts';
import catalog from '../../../../api/src/desktop_settings/model-catalog.json' with { type: 'json' };
import { isMac } from '../settings/platform';

/** Where on-device inference runs, named for this machine. */
export const hereLabel = () => (isMac() ? 'on this Mac' : 'on this PC');

const NAMES = new Map(catalog.map((model) => [model.id, model.display_name]));
/** The catalog's display name for a local model id; cloud ids pass through. */
export const modelName = (id: string) => NAMES.get(id) ?? id;
/** A short name for tight places such as the status pill and the dock. */
export const shortModel = (id: string) => modelName(id).replace(/^Whisper large-v3-turbo/, 'Whisper turbo').replace(/^ivrit\.ai Whisper large-v3-turbo/, 'ivrit.ai turbo');

/**
 * Direction is a property of a transcript, not of the application.
 *
 * A Hebrew session renders `rtl`, an English one `ltr`, and a mixed one `auto`
 * so the first strong character decides and bidi runs resolve inside it. The
 * session list holds both at once; that is the normal case here.
 */
export function directionFor(language: InputLanguage): 'ltr' | 'rtl' | 'auto' {
  return language === 'he' ? 'rtl' : language === 'mixed' ? 'auto' : 'ltr';
}

export function languageLabel(language: InputLanguage): string {
  return language === 'he' ? 'עברית' : language === 'mixed' ? 'English + עברית' : 'English';
}

/** Where inference actually happens — never a vague "local" that blurs the two. */
export function executionLabel(provider: string, integrated: boolean): string {
  if (provider !== 'local') return provider === 'openai' ? 'OpenAI' : provider === 'gemini' ? 'Gemini' : provider;
  return integrated ? hereLabel() : 'on your server';
}

/**
 * A text model's id as a name: `gemini-3.8-flash` → "Gemini 3.8 Flash",
 * `openai/gpt-5-mini` → "GPT-5 mini". Only for display; ids stay ids everywhere else.
 */
export function textModelName(id: string): string {
  const base = id.includes('/') ? id.slice(id.lastIndexOf('/') + 1) : id;
  const gpt = /^gpt-([\w.]+)(.*)$/i.exec(base);
  if (gpt) return `GPT-${gpt[1]}${gpt[2].replace(/-/g, ' ')}`;
  // Consecutive numbers are one version: "3-5" is 3.5.
  return base.split('-').reduce<string[]>((words, part) => {
    const previous = words[words.length - 1];
    if (/^\d+$/.test(part) && previous && /^\d+(\.\d+)?$/.test(previous)) words[words.length - 1] = `${previous}.${part}`;
    else words.push(/^[a-z]/.test(part) ? part[0].toUpperCase() + part.slice(1) : part);
    return words;
  }, []).join(' ');
}
