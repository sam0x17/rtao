// Resolve reviewed dependency candidates on the hosted runner.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

if (process.env.GITHUB_ACTIONS !== 'true'
    || process.env.RUNNER_ENVIRONMENT !== 'github-hosted'
    || process.env.RUNNER_OS !== 'Linux'
    || process.env.RUNNER_ARCH !== 'X64') {
  throw new Error('Lock refresh is restricted to hosted Ubuntu CI.');
}

// The first pinned docs-tool resolution has its own bounded candidate guard.
if (readFileSync('Cargo.toml', 'utf8').includes('\ndocify = "=0.2.9"\n')
    && !/name = "docify"\nversion = "0.2.9"/.test(readFileSync('Cargo.lock', 'utf8'))) {
  execFileSync(process.execPath, ['ci/resolve-docify-lock.mjs'], {stdio:'inherit',timeout:170000});
  process.exit(0);
}

// This precise compatible update removes the release yanked on 2026-10-16.
if (/name = "inout"\nversion = "0.2.3"\n/.test(readFileSync('Cargo.lock', 'utf8'))) {
  execFileSync(process.execPath, ['ci/resolve-inout-lock.mjs'], {stdio:'inherit',timeout:170000});
  process.exit(0);
}

function packages(text) {
  const result = new Map();
  for (const body of text.split('[[package]]').slice(1)) {
    const field = name => body.match(new RegExp(`^${name} = "([^"]+)"`, 'm'))?.[1] ?? '';
    const name = field('name');
    const version = field('version');
    const source = field('source');
    const checksum = field('checksum');
    const identity = [name, version, source, checksum].join('|');
    if (!name || result.has(identity)) throw new Error('Missing or duplicate lock identity.');
    const block = body.match(/^dependencies = \[\n([\s\S]*?)^\]\n?/m);
    const dependencies = block ? block[1].split('\n').filter(Boolean).map(line => {
      const match = line.match(/^ "([^"\n]+)",$/);
      if (!match) throw new Error('Unexpected dependency record.');
      return match[1];
    }) : [];
    result.set(identity, {
      name, version, source, checksum, identity, body, dependencies,
      rest: block ? body.replace(block[0], '') : body,
    });
  }
  if (result.size < 100) throw new Error('Unexpectedly small lock inventory.');
  return result;
}

const additions = new Map([
  ['rtao-cli', 'nix'],
  ['rtao-testkit', 'docify'],
  ['rtao-wallet', 'rustix'],
  ['rtao-rpc', 'tokio-rustls'],
]);
const pinText = readFileSync('upstream.toml', 'utf8');
const corePatch = pinText.match(/\[core\.patch\]\n[\s\S]*?^rev = "([0-9a-f]{40})"$/m);
if (!corePatch) throw new Error('Missing immutable core.patch revision.');
const expectedCoreSource = `git+https://github.com/sam0x17/subtensor.git?rev=${corePatch[1]}#${corePatch[1]}`;
if (!readFileSync('Cargo.toml', 'utf8').includes(
  `bittensor-core = { git = "https://github.com/sam0x17/subtensor.git", rev = "${corePatch[1]}" }`
)) throw new Error('Cargo.toml and core.patch revisions differ.');

function withoutSource(rest) {
  return rest.replace(/^source = "[^"]+"\n/m, '');
}

const deadline = Date.now() + 170000;
const reports = [];
for (const [lock, manifest] of [['Cargo.lock', 'Cargo.toml'], ['fuzz/Cargo.lock', 'fuzz/Cargo.toml']]) {
  const beforeText = execFileSync('git', ['show', `HEAD:${lock}`], { encoding: 'utf8', maxBuffer: 1048576 });
  if (readFileSync(lock, 'utf8') !== beforeText) throw new Error(`Lock refresh requires its committed baseline: ${lock}`);
  const before = packages(beforeText);
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw new Error('Lock refresh deadline elapsed.');
  execFileSync('cargo', ['update', '--workspace', '--manifest-path', manifest], { stdio: 'inherit', timeout: remaining });
  const after = packages(readFileSync(lock, 'utf8'));
  if (before.size !== after.size) throw new Error(`Lock refresh changed package inventory: ${lock}`);
  const changed = [];
  const consumed = new Set();
  for (const [identity, old] of before) {
    let current = after.get(identity);
    let sourceChanged = false;
    if (!current && old.name === 'bittensor-core'
        && old.source.startsWith('git+https://github.com/sam0x17/subtensor.git?rev=')) {
      const candidates = [...after.values()].filter(value => value.name === old.name
        && value.version === old.version && value.source === expectedCoreSource
        && value.checksum === old.checksum);
      if (candidates.length !== 1) throw new Error(`Missing unique reviewed core source replacement: ${lock}`);
      current = candidates[0];
      sourceChanged = true;
      if (withoutSource(old.rest) !== withoutSource(current.rest)) {
        throw new Error(`Core source replacement changed other package metadata: ${lock}`);
      }
    }
    if (!current || (!sourceChanged && old.rest !== current.rest)) {
      throw new Error(`Changed package identity or metadata: ${identity}`);
    }
    if (consumed.has(current.identity)) throw new Error(`Duplicate refreshed package match: ${current.identity}`);
    consumed.add(current.identity);
    const added = current.dependencies.filter(value => !old.dependencies.includes(value));
    const removed = old.dependencies.filter(value => !current.dependencies.includes(value));
    const allowed = sourceChanged ? 'ed25519-zebra' : additions.get(old.name);
    if (removed.length || added.some(value => value !== allowed)) {
      throw new Error(`Unreviewed dependency edge change: ${old.name}`);
    }
    if (sourceChanged || added.length) {
      changed.push({ package: old.name, source: sourceChanged ? expectedCoreSource : undefined, added });
    }
  }
  if (consumed.size !== after.size) throw new Error(`Unreviewed package appeared in ${lock}`);
  reports.push({ lock, changed, packages: after.size });
}
console.log(JSON.stringify({
  reports,
  claim: 'Existing package identities are preserved except the authenticated core.patch revision; candidate locks still require source review.',
}));
