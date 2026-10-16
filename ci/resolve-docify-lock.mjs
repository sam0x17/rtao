// Generate a candidate for the plan's pinned documentation tool, in hosted CI only.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

if (process.env.GITHUB_ACTIONS !== 'true'
    || process.env.RUNNER_ENVIRONMENT !== 'github-hosted'
    || process.env.RUNNER_OS !== 'Linux'
    || process.env.RUNNER_ARCH !== 'X64') {
  throw new Error('Docify lock resolution requires free hosted Ubuntu CI.');
}
const git = args => execFileSync('git', args, {encoding: 'utf8', maxBuffer: 1048576});
const committedText = git(['show', 'HEAD:Cargo.lock']);
// Compare to the unchanged pre-Docify graph; the failed 0.4.1 tool closure may be removed.
const beforeText = git(['show', '80ad78b479b957236003272069ae6b4410797e67:Cargo.lock']);
if (createHash('sha256').update(beforeText).digest('hex')
    !== '6cd0d7a767173f6c558ab10092e81902ba6a3a977c1e5b36dc427685b906bde1') {
  throw new Error('Pre-Docify baseline identity changed.');
}
const fuzz = git(['show', 'HEAD:fuzz/Cargo.lock']);
if (git(['status', '--porcelain=v1']) || readFileSync('Cargo.lock', 'utf8') !== committedText
    || readFileSync('fuzz/Cargo.lock', 'utf8') !== fuzz) {
  throw new Error('Docify resolution requires clean committed inputs and locks.');
}
function dependency(file, section, expected) {
  const source = readFileSync(file, 'utf8');
  const header = '[' + section + ']\n';
  const pieces = source.split(header);
  if (pieces.length !== 2) throw new Error('One dependency section required: ' + file);
  const body = pieces[1].split(/^\[/m)[0];
  const declarations = source.match(/^docify(?:\.workspace)? = .*$/gm) ?? [];
  if (declarations.length !== 1 || declarations[0] !== expected
      || !body.split('\n').includes(expected)) {
    throw new Error('Unexpected Docify declaration: ' + file);
  }
}
dependency('Cargo.toml', 'workspace.dependencies', 'docify = "=0.2.9"');
dependency('crates/rtao/Cargo.toml', 'dev-dependencies', 'docify.workspace = true');
dependency('xtask/Cargo.toml', 'dependencies', 'docify.workspace = true');

function packages(text) {
  const result = new Map();
  for (const body of text.split('[[package]]').slice(1)) {
    const field = key => body.match(new RegExp('^' + key + ' = "([^"\n]+)"', 'm'))?.[1] ?? '';
    const name = field('name'), version = field('version'), source = field('source'), checksum = field('checksum');
    const identity = [name, version, source, checksum].join('|');
    const block = body.match(/^dependencies = \[\n([\s\S]*?)^\]\n?/m);
    const dependencies = block ? block[1].split('\n').filter(Boolean).map(line => {
      const match = line.match(/^ "([^"\n]+)",$/);
      if (!match) throw new Error('Unexpected lock dependency syntax.');
      return match[1];
    }) : [];
    if (!name || !version || result.has(identity)) throw new Error('Invalid package identity.');
    result.set(identity, {name, version, source, checksum, dependencies, rest: block ? body.replace(block[0], '') : body});
  }
  if (result.size < 100) throw new Error('Unexpectedly small package inventory.');
  return result;
}
const before = packages(beforeText);
if ([...before.values()].some(p => p.name === 'docify' || p.name === 'docify_macros')) {
  throw new Error('This preparation only admits the first pinned Docify declaration.');
}
execFileSync('cargo', ['update', '--workspace', '--manifest-path', 'Cargo.toml'], {
  stdio: 'inherit', timeout: 150000,
});
const after = packages(readFileSync('Cargo.lock', 'utf8'));
// Exact local-snippet tool closure from the published crates.io index.
// No remote snippet feature, Git library or new native build helper is required.
const expectedAdditions = new Map([
  ['common-path', ['1.0.0', '2382f75942f4b3be3690fe4f86365e9c853c1587d6ee58212cebf6e2a9ccd101']],
  ['docify', ['0.2.9', 'a772b62b1837c8f060432ddcc10b17aae1453ef17617a99bc07789252d2a5896']],
  ['docify_macros', ['0.2.9', '60e6be249b0a462a14784a99b19bf35a667bb5e09de611738bb7362fa4c95ff7']],
  ['serde_spanned', ['0.6.9', 'bf41e0cfaf7226dca15e8197172c295a782857fcb97fad1808a166870dee75a3']],
  ['termcolor', ['1.4.1', '06794f8f6c5c898b3275aebefa6b8a1cb24cd2c6c79397ab15774837a0bc5755']],
  ['toml', ['0.8.23', 'dc1beb996b9d83529a9e75c17a1686767d148d70663143c7854d8b4a09ced362']],
  ['toml_datetime', ['0.6.11', '22cddaf88f4fbc13c51aebbf5f8eceb5c7c5a9da2ac40a13519eb5b0a0e8f11c']],
  ['toml_edit', ['0.22.27', '41fe8c660ae4257887cf66394862d21dbca4a6ddd26f04a3560410406a2f819a']],
  ['toml_write', ['0.1.2', '5d99f8c9a7727884afe522e9bd5edbfc91a3312b36a77b5fb8926e4c31a41801']],
]);
const added = [...after].filter(([identity]) => !before.has(identity));
console.log(JSON.stringify({candidateNewPackages: added.map(([, p]) => ({name:p.name,version:p.version,source:p.source,checksum:p.checksum}))}));
if (added.length !== expectedAdditions.size) throw new Error('Unexpected Docify package count.');
for (const [, current] of added) {
  const expected = expectedAdditions.get(current.name);
  if (!expected || current.version !== expected[0] || current.checksum !== expected[1]
      || current.source !== 'registry+https://github.com/rust-lang/crates.io-index') {
    throw new Error('Unreviewed new documentation package: ' + current.name);
  }
}
function identity(record) {
  return [record.name, record.version, record.source, record.checksum].join('|');
}
function resolve(edge, records) {
  const match = edge.match(/^([^ ]+)(?: ([0-9][^ ]*))?(?: \(([^)]+)\))?$/);
  if (!match) throw new Error('Unexpected dependency reference: ' + edge);
  const [, name, version, source] = match;
  const candidates = [...records.values()].filter(p => p.name === name
    && (!version || p.version === version)
    && (!source || p.source === source || p.source.split('#')[0] === source));
  if (candidates.length !== 1) throw new Error('Ambiguous dependency reference: ' + edge);
  return candidates[0];
}
const docifyRoots = [...after.values()].filter(p => p.name === 'docify' && p.version === '0.2.9');
if (docifyRoots.length !== 1) throw new Error('Exactly one pinned Docify root required.');
const docifyIdentity = identity(docifyRoots[0]);
const changed = [];
for (const [key, old] of before) {
  const current = after.get(key);
  if (!current || current.rest !== old.rest) throw new Error('Existing identity/metadata changed: ' + old.name);
  // Cargo may qualify unchanged references when a second version is added.
  // Preserve resolved identities, including source and checksum, across that rewrite.
  const oldEdges = old.dependencies.map(edge => identity(resolve(edge, before)));
  const currentEdges = current.dependencies.map(edge => identity(resolve(edge, after)));
  const newEdges = currentEdges.filter(value => !oldEdges.includes(value));
  const removed = oldEdges.filter(value => !currentEdges.includes(value));
  const allowed = value => ['rtao', 'xtask'].includes(old.name) && value === docifyIdentity;
  if (removed.length || newEdges.some(value => !allowed(value))) {
    throw new Error('Unexpected existing dependency edge change: ' + old.name);
  }
  if (newEdges.length) changed.push({name:old.name,added:newEdges});
}
if (JSON.stringify(changed.map(p => p.name).sort()) !== JSON.stringify(['rtao', 'xtask'])) {
  throw new Error('Only the facade dev dependency and xtask dependency may gain Docify.');
}
const closure = new Set();
function visit(record) {
  const key = identity(record);
  if (closure.has(key)) return;
  closure.add(key);
  for (const edge of record.dependencies) {
    visit(resolve(edge, after));
  }
}
visit(docifyRoots[0]);
if (added.some(([identity]) => !closure.has(identity))) throw new Error('Added package is outside Docify closure.');
if (readFileSync('fuzz/Cargo.lock', 'utf8') !== fuzz
    || git(['diff', '--name-only']).trim() !== 'Cargo.lock') {
  throw new Error('Only the root lock may change during documentation preparation.');
}
console.log(JSON.stringify({changed, newPackages:added.length, existingIdentitiesPreserved:before.size,
  claim:'Candidate only; exact source/lock review, graph regeneration and full hosted supply/tests remain required.'}));
