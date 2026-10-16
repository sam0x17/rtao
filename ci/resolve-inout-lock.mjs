// Replace the newly yanked inout release with its compatible published patch.
// Cargo resolves the candidate on a free hosted runner; nothing is adopted here.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

if (process.env.GITHUB_ACTIONS !== 'true'
    || process.env.RUNNER_ENVIRONMENT !== 'github-hosted'
    || process.env.RUNNER_OS !== 'Linux'
    || process.env.RUNNER_ARCH !== 'X64') {
  throw new Error('inout lock resolution requires free hosted Ubuntu CI.');
}
const git = args => execFileSync('git', args, {encoding: 'utf8', maxBuffer: 1048576});
const before = git(['show', 'HEAD:Cargo.lock']);
const fuzz = git(['show', 'HEAD:fuzz/Cargo.lock']);
if (git(['status', '--porcelain=v1']) || readFileSync('Cargo.lock', 'utf8') !== before
    || readFileSync('fuzz/Cargo.lock', 'utf8') !== fuzz) {
  throw new Error('inout resolution requires clean committed inputs and locks.');
}
const oldIdentity = 'name = "inout"\nversion = "0.2.3"\n'
  + 'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
  + 'checksum = "d8c0d789294fb457b791f7797a04478704788f30b407e5d0b01deaa846572d9f"';
const newIdentity = 'name = "inout"\nversion = "0.2.4"\n'
  + 'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
  + 'checksum = "3ce983bb6fb399067d101fbfc9e74174e07e9f29a2bc679efefeb8c645819c3f"';
const oldEdge = ' "inout 0.2.3",\n';
if (before.split(oldIdentity).length !== 2 || before.split(oldEdge).length !== 2
    || before.includes(newIdentity) || /name = "inout"\nversion = "0.2.3"\n/.test(fuzz)) {
  throw new Error('Reviewed inout identity or sole dependency edge differs.');
}
const expected = before.replace(oldIdentity, newIdentity)
  .replace(oldEdge, ' "inout 0.2.4",\n');
execFileSync('cargo', ['update', '--manifest-path', 'Cargo.toml',
  '--package', 'inout@0.2.3', '--precise', '0.2.4'], {
  stdio: 'inherit', timeout: 150000,
});
const after = readFileSync('Cargo.lock', 'utf8');
if (after !== expected || readFileSync('fuzz/Cargo.lock', 'utf8') !== fuzz) {
  throw new Error('Candidate changed more than the reviewed inout identity and edge.');
}
const changed = git(['diff', '--name-only']).trim().split('\n');
if (changed.length !== 1 || changed[0] !== 'Cargo.lock'
    || git(['ls-files', '--others', '--exclude-standard'])) {
  throw new Error('Candidate changed an unexpected path.');
}
const sha = text => createHash('sha256').update(text).digest('hex');
console.log(JSON.stringify({candidate: 'Cargo.lock', beforeSha256: sha(before),
  afterSha256: sha(after), oldVersion: '0.2.3', newVersion: '0.2.4',
  checksum: '3ce983bb6fb399067d101fbfc9e74174e07e9f29a2bc679efefeb8c645819c3f',
  fuzzUnchanged: true, claim: 'Candidate only; artifact review and full hosted gates remain required.'}));
