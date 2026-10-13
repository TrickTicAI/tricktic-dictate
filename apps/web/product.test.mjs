import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';

// index.html cannot import the constant, so it is checked against it here.
test('the page title is the public product name', async () => {
  const product = await readFile(new URL('./src/product.ts', import.meta.url), 'utf8');
  const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');
  const name = product.match(/PRODUCT_NAME = '([^']+)'/)?.[1];
  assert.ok(name);
  assert.equal(html.match(/<title>([^<]+)<\/title>/)?.[1], name);
});
