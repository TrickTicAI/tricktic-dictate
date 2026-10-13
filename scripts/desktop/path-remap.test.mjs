import test from 'node:test';
import assert from 'node:assert/strict';
import { remapEnv, remapRoots, cRemapFlags } from './path-remap.mjs';

const base = { repo: '/h/me/src/writer', home: '/h/me', sysroot: '/h/me/.rustup/toolchains/stable' };

test('home is remapped first so specific roots win (rustc applies the last match)', () => {
  const roots = remapRoots({ ...base, env: {} });
  assert.deepEqual(roots[0], ['/h/me', '~']);
  assert.deepEqual(roots.at(-1), ['/h/me/src/writer/target', '/target']);
  assert.ok(roots.some(([from, to]) => from === '/h/me/.cargo' && to === '/cargo'));
  assert.ok(roots.some(([from, to]) => from === base.sysroot && to === '/rustc-sysroot'));
});

test('target rustflags are extended, never replaced, and Windows keeps only the PDB file name', () => {
  const env = { CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS: '-L native=/sdk' };
  const result = remapEnv({ ...base, env, target: 'x86_64-pc-windows-msvc', windows: true, compiler: 'clang-cl' });
  const flags = result.CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS;
  assert.ok(flags.startsWith('-L native=/sdk --remap-path-prefix=/h/me=~ '));
  assert.ok(flags.endsWith('-Clink-arg=/PDBALTPATH:%_PDB%'));
  assert.equal(result.RUSTFLAGS, undefined);
  assert.match(result.CFLAGS_x86_64_pc_windows_msvc, /^\/clang:-ffile-prefix-map=\/h\/me=~ /);
});

test('an existing RUSTFLAGS hides target rustflags in Cargo, so the remap joins it', () => {
  const result = remapEnv({ ...base, env: { RUSTFLAGS: '-Ctarget-cpu=apple-m1' }, target: 'aarch64-apple-darwin', windows: false });
  assert.ok(result.RUSTFLAGS.startsWith('-Ctarget-cpu=apple-m1 --remap-path-prefix='));
  assert.equal(result.CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS, undefined);
  assert.ok(!result.RUSTFLAGS.includes('PDBALTPATH'));
  assert.match(result.CFLAGS_aarch64_apple_darwin, /^-ffile-prefix-map=\/h\/me=~ /);
});

test('native MSVC trims __FILE__ prefixes', () => {
  assert.deepEqual(cRemapFlags([['C:\\Users\\me', '~']], 'cl'), ['/d1trimfile:C:\\Users\\me\\']);
});
