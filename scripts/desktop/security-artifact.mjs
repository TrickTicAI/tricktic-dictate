// Public package gate: inspect the compiled application and mapped resource names.
import { readFile } from 'node:fs/promises';
export function assertSupportedDesktopResources(paths) {
  for (const path of paths) {
    if (/(^|\/)(MetrixWriter(?:\.[^/]+)?\.exe(?:\.config)?|Microsoft\.Web\.WebView2\.[^/]*\.dll|resources\.json|[^/]+\.cs)$/i.test(path.replaceAll('\\', '/'))
      || /(^|\/)(backend|windows)\//i.test(path)) throw Error(`Retired desktop entry point in package: ${path}`);
  }
}
export async function assertNoDesktopFixture(executable) {
  const bytes = await readFile(executable);
  for (const marker of ['writerSecurityFixture', 'security_probe_receipt']) {
    if (bytes.includes(Buffer.from(marker)) || bytes.includes(Buffer.from(marker, 'utf16le'))) {
      throw Error('Security fixture code is present in a public desktop binary. Rebuild without probe features.');
    }
  }
}

// Sign-in remains optional for development builds. Check both enabled and
// disabled artifacts so packaging cannot silently lose the requested feature.
const SIGN_IN_MARKERS = ['accounts.google.com/o/oauth2/v2/auth', 'securetoken.googleapis.com'];
export async function assertNoGoogleSignIn(executable) {
  const bytes = await readFile(executable);
  for (const marker of SIGN_IN_MARKERS) {
    if (bytes.includes(Buffer.from(marker)) || bytes.includes(Buffer.from(marker, 'utf16le'))) {
      throw Error('Google sign-in code is present in a desktop binary built without --google-sign-in.');
    }
  }
}

// Reads the owner's private sign-in configuration (the MW-OUATH-2 proof format)
// into the build environment the `google-sign-in` feature compiles in. Values
// are never printed. Releases require the account deletion/revocation service.
export async function googleSignInEnv({ path, release }) {
  let config;
  try { config = JSON.parse(await readFile(path, 'utf8')); } catch { throw Error('The --google-sign-in configuration could not be read as JSON.'); }
  const text = value => typeof value === 'string' && value.length > 0 && value.length <= 256 && /^[\x21-\x7e]+$/.test(value);
  const client = /^[A-Za-z0-9_-]+\.apps\.googleusercontent\.com$/;
  if (!config || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(config.projectId) || !text(config.apiKey) || config.apiKey.includes('REPLACE')
      || !client.test(config.desktopClientId) || (config.desktopClientSecret !== undefined && !text(config.desktopClientSecret))
      || (config.accountServiceUrl !== undefined && !serviceUrl(config.accountServiceUrl))) {
    throw Error('The --google-sign-in configuration needs projectId, apiKey and desktopClientId (desktopClientSecret and an https accountServiceUrl optional); see infra/firebase/README.md.');
  }
  if (release && (!config.accountServiceUrl || !config.accountServiceUrl.startsWith('https://')
      || ['localhost', '127.0.0.1', '[::1]'].includes(new URL(config.accountServiceUrl).hostname))) {
    throw Error('Desktop releases require a public HTTPS accountServiceUrl for account deletion and revocation.');
  }
  return {
    WRITER_GOOGLE_PROJECT_ID: config.projectId,
    WRITER_FIREBASE_API_KEY: config.apiKey,
    WRITER_GOOGLE_DESKTOP_CLIENT_ID: config.desktopClientId,
    WRITER_GOOGLE_DESKTOP_CLIENT_SECRET: config.desktopClientSecret ?? '',
    // The trusted cleanup service (MW-OUATH-8). Without it, deletion and
    // session revocation report that this build is not configured.
    WRITER_ACCOUNT_SERVICE_URL: config.accountServiceUrl ?? '',
  };
}
function serviceUrl(value) {
  try {
    const url = new URL(value);
    const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname);
    return (url.protocol === 'https:' || (url.protocol === 'http:' && loopback)) && !url.username && !url.password && !url.search && !url.hash;
  } catch { return false; }
}
export const GOOGLE_SIGN_IN_ENV = ['WRITER_GOOGLE_PROJECT_ID', 'WRITER_FIREBASE_API_KEY', 'WRITER_GOOGLE_DESKTOP_CLIENT_ID', 'WRITER_GOOGLE_DESKTOP_CLIENT_SECRET', 'WRITER_ACCOUNT_SERVICE_URL'];

// Resolve only a path, never configuration values, from the caller's environment.
// Explicit feature configuration also prevents an update from dropping sign-in.
export function desktopSignInConfig({ path, release, env = process.env }) {
  const selected = path ?? env.WRITER_GOOGLE_SIGN_IN_CONFIG;
  if (selected !== undefined && (typeof selected !== 'string' || !selected.trim() || selected.startsWith('--'))) {
    throw Error('Pass a private configuration path to --google-sign-in or WRITER_GOOGLE_SIGN_IN_CONFIG.');
  }
  if (release && !selected) throw Error('Desktop releases require --google-sign-in <private config> or WRITER_GOOGLE_SIGN_IN_CONFIG; refusing a release without account login.');
  return selected;
}
export async function assertGoogleSignIn(executable) {
  const bytes = await readFile(executable);
  for (const marker of SIGN_IN_MARKERS) {
    if (!bytes.includes(Buffer.from(marker)) && !bytes.includes(Buffer.from(marker, 'utf16le'))) {
      throw Error('Requested Google sign-in is missing from the desktop binary.');
    }
  }
}
