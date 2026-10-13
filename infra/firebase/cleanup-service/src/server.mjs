// Entry point: `node src/server.mjs`. Listens on loopback by default; an HTTPS
// reverse proxy is the only public entry (see README.md).
import { createServer } from 'node:http';
import { applicationDefault, initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { readConfig } from './config.mjs';
import { createService, jsonLog } from './service.mjs';

const config = readConfig(process.env);
const app = initializeApp({ projectId: config.projectId,
  ...(!config.emulated && { credential: applicationDefault() }) });
const service = createService({ projectId: config.projectId, auth: getAuth(app), db: getFirestore(app) });
const server = createServer(service.handle);
server.requestTimeout = 30_000;
server.headersTimeout = 10_000;
server.listen(config.port, config.host, () => {
  jsonLog('listening', { port: config.port, emulated: config.emulated });
  service.start();
});
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    service.stop();
    server.close(() => process.exit(0));
  });
}
