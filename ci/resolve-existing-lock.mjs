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
    const identity = [name, field('version'), field('source'), field('checksum')].join('|');
    if (!name || result.has(identity)) throw new Error('Missing or duplicate lock identity.');
    const block = body.match(/^dependencies = \[\n([\s\S]*?)^\]\n?/m);
    const dependencies = block ? block[1].split('\n').filter(Boolean).map(line => {
      const match = line.match(/^ "([^"\n]+)",$/);
      if (!match) throw new Error('Unexpected dependency record.');
      return match[1];
    }) : [];
    result.set(identity, { name, body, dependencies, rest: block ? body.replace(block[0], '') : body });
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
  for (const [identity, old] of before) {
    const current = after.get(identity);
    if (!current || old.rest !== current.rest) throw new Error(`Changed package identity or metadata: ${identity}`);
    const added = current.dependencies.filter(value => !old.dependencies.includes(value));
    const removed = old.dependencies.filter(value => !current.dependencies.includes(value));
    if (removed.length || added.some(value => value !== additions.get(old.name))) {
      throw new Error(`Unreviewed dependency edge change: ${old.name}`);
    }
    if (added.length) changed.push({ package: old.name, added });
  }
  reports.push({ lock, changed, packages: after.size });
}
console.log(JSON.stringify({ reports, claim: 'Existing package identities preserved; candidate locks still require source review.' }));
