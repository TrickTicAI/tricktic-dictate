// Service configuration from the environment. No credential is ever read from
// a flag or logged; GOOGLE_APPLICATION_CREDENTIALS names a key file only.
const project = /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/;

export function readConfig(env) {
  const projectId = env.WRITER_FIREBASE_PROJECT_ID ?? '';
  if (!project.test(projectId)) throw new Error('WRITER_FIREBASE_PROJECT_ID is missing or invalid');
  const emulators = [env.FIRESTORE_EMULATOR_HOST, env.FIREBASE_AUTH_EMULATOR_HOST].filter(Boolean).length;
  if (emulators === 1) throw new Error('Set both Firestore and Auth emulator hosts, or neither');
  const emulated = emulators === 2;
  if (emulated && !projectId.startsWith('demo-')) throw new Error('Emulators run only with a demo- project');
  if (!emulated && !env.GOOGLE_APPLICATION_CREDENTIALS) throw new Error('GOOGLE_APPLICATION_CREDENTIALS is not set');
  const port = Number(env.PORT ?? 8787);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('PORT is invalid');
  return { projectId, emulated, host: env.HOST ?? '127.0.0.1', port };
}
