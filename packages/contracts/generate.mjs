import { compile, compileFromFile } from 'json-schema-to-typescript';
import { readFile, writeFile } from 'node:fs/promises';
import accountSync from './account-sync.schema.mjs';

const types = await compileFromFile('schema.json', {
  bannerComment: '/* Generated from Rust via schema.json. Run pnpm contracts:generate. */',
  unreachableDefinitions: true,
});
if (process.argv.includes('--write')) await writeFile('index.d.ts', types);
else if (await readFile('index.d.ts', 'utf8') !== types) {
  throw new Error('TypeScript contracts are stale. Run pnpm contracts:generate.');
}

const accountTypes = await compile(accountSync, 'AccountSyncCatalog', {
  bannerComment: '/* Generated from account-sync.schema.mjs. Run node generate.mjs --write. */',
  unreachableDefinitions: true,
});
for (const [path, value] of [
  ['account-sync.schema.json', `${JSON.stringify(accountSync, null, 2)}\n`],
  ['account-sync.d.ts', accountTypes],
]) {
  if (process.argv.includes('--write')) await writeFile(path, value);
  else if (await readFile(path, 'utf8') !== value) {
    throw new Error(`${path} is stale. Run node generate.mjs --write in packages/contracts.`);
  }
}
