// Keep build-machine paths out of shipped binaries (MW-RELEASE-14).
// Rust panic locations and C `__FILE__` strings otherwise embed the builder's
// home folder, Cargo registry and checkout paths. rustc applies the last
// matching --remap-path-prefix, so the catch-all home mapping comes first.
import { spawnSync } from 'node:child_process';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

export function remapRoots({ repo, env = process.env, home = homedir(), sysroot = rustSysroot(env), targetDir }) {
  const roots = [[home, '~']];
  roots.push([resolve(repo), '/writer']);
  roots.push([resolve(env.CARGO_HOME ?? resolve(home, '.cargo')), '/cargo']);
  if (env.RUSTUP_HOME) roots.push([resolve(env.RUSTUP_HOME), '/rustup']);
  if (sysroot) roots.push([sysroot, '/rustc-sysroot']);
  roots.push([resolve(targetDir ?? env.CARGO_TARGET_DIR ?? resolve(repo, 'target')), '/target']);
  return roots;
}

export function rustSysroot(env = process.env) {
  const result = spawnSync('rustc', ['--print', 'sysroot'], { env, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : undefined;
}

export const rustRemapFlags = roots => roots.map(([from, to]) => `--remap-path-prefix=${from}=${to}`);

// cc-rs compiles C dependencies with these. clang-cl takes clang options through
// /clang:; native MSVC cl.exe only trims __FILE__ prefixes (/d1trimfile).
export function cRemapFlags(roots, compiler) {
  if (compiler === 'cl') return roots.map(([from]) => `/d1trimfile:${from}\\`);
  const flags = roots.map(([from, to]) => `-ffile-prefix-map=${from}=${to}`);
  return compiler === 'clang-cl' ? flags.map(flag => `/clang:${flag}`) : flags;
}

const append = (current, flags) => [current, ...flags].filter(Boolean).join(' ');

// Rust flags are not merged by Cargo: RUSTFLAGS hides target rustflags, which
// the WSL runner uses for the Windows SDK. Extend whichever source is in use.
export function remapEnv({ repo, env = process.env, target, windows, compiler, ...rest }) {
  const roots = remapRoots({ repo, env, ...rest });
  const rust = rustRemapFlags(roots);
  // MSVC-style linkers record the absolute .pdb path; keep only its file name.
  if (windows) rust.push('-Clink-arg=/PDBALTPATH:%_PDB%');
  const result = {};
  if (env.CARGO_ENCODED_RUSTFLAGS) result.CARGO_ENCODED_RUSTFLAGS = [env.CARGO_ENCODED_RUSTFLAGS, ...rust].join('\x1f');
  else if (env.RUSTFLAGS) result.RUSTFLAGS = append(env.RUSTFLAGS, rust);
  else {
    const triple = (target ?? hostTriple(env)).toUpperCase().replaceAll('-', '_');
    result[`CARGO_TARGET_${triple}_RUSTFLAGS`] = append(env[`CARGO_TARGET_${triple}_RUSTFLAGS`], rust);
  }
  const c = cRemapFlags(roots, compiler ?? (windows ? (process.platform === 'win32' ? 'cl' : 'clang-cl') : 'clang'));
  const cTriple = (target ?? hostTriple(env)).replaceAll('-', '_');
  for (const name of [`CFLAGS_${cTriple}`, `CXXFLAGS_${cTriple}`]) result[name] = append(env[name], c);
  return result;
}

export function hostTriple(env = process.env) {
  const result = spawnSync('rustc', ['-vV'], { env, encoding: 'utf8' });
  const host = result.status === 0 && result.stdout.match(/^host: (\S+)$/m);
  if (!host) throw Error('rustc -vV did not report a host target.');
  return host[1];
}
