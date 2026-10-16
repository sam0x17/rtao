// Generate a candidate for the plan's pinned documentation tool, in hosted CI only.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

if (process.env.GITHUB_ACTIONS !== 'true'
    || process.env.RUNNER_ENVIRONMENT !== 'github-hosted'
    || process.env.RUNNER_OS !== 'Linux'
    || process.env.RUNNER_ARCH !== 'X64') {
  throw new Error('Docify lock resolution requires free hosted Ubuntu CI.');
}
const git = args => execFileSync('git', args, {encoding: 'utf8', maxBuffer: 1048576});
const beforeText = git(['show', 'HEAD:Cargo.lock']);
const fuzz = git(['show', 'HEAD:fuzz/Cargo.lock']);
if (git(['status', '--porcelain=v1']) || readFileSync('Cargo.lock', 'utf8') !== beforeText
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
dependency('Cargo.toml', 'workspace.dependencies', 'docify = "=0.4.1"');
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
// Exact additions observed in authenticated hosted candidate 38891932293.
// Candidate identities still require lock/source review and full supply checks.
const expectedAdditions = new Map([
  ['common-path', ['1.0.0', '2382f75942f4b3be3690fe4f86365e9c853c1587d6ee58212cebf6e2a9ccd101']],
  ['docify', ['0.4.1', 'bff55a6155fef558216361d920106abd26d6235a80830b881c656a85eb021981']],
  ['docify_macros', ['0.4.1', 'e79a94eab471940a3ae1b644869c8b39e57a7f15b4e004c54b57894632e26b02']],
  ['git2', ['0.21.0', 'ddddbf932745a6be37109b6112d3ee09696106f848449069d3a57bba937ab82e']],
  ['jobserver', ['0.1.35', '1c00acbd29eabad4a2392fa0e921c874934dbbf4194312ad20f04a0ed67a3cb3']],
  ['libgit2-sys', ['0.18.8+1.9.7', '7f7c568b25d7489bc3fb2988ed69ab111d2944d2f5fec3d5c987fe545ea97b50']],
  ['libz-sys', ['1.1.30', 'f710a23e6dbf193214fd46ca56a9d6864e550abe86202184532ae7275e46de19']],
  ['serde_spanned', ['1.1.2', '4e7523beb55eece201a2356bee0bbca0d1ab466c14c07703b2e0ee6d42cb0c2c']],
  ['termcolor', ['1.4.1', '06794f8f6c5c898b3275aebefa6b8a1cb24cd2c6c79397ab15774837a0bc5755']],
  ['toml', ['0.9.12+spec-1.1.0', 'cf92845e79fc2e2def6a5d828f0801e29a2f8acc037becc5ab08595c7d5e9863']],
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
const docifyRoots = [...after.values()].filter(p => p.name === 'docify' && p.version === '0.4.1');
if (docifyRoots.length !== 1) throw new Error('Exactly one pinned Docify root required.');
const docifyIdentity = identity(docifyRoots[0]);
// libgit2-sys enables the existing cc build tool's parallel feature.
// Permit only its optional libc/jobserver build edges; its identity stays fixed.
const buildEdgeTargets = new Set([...after.values()].filter(p =>
  (p.name === 'jobserver' && p.version === '0.1.35')
  || (p.name === 'libc' && before.has(identity(p)))).map(identity));
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
  const allowed = value => (['rtao', 'xtask'].includes(old.name) && value === docifyIdentity)
    || (old.name === 'cc' && old.version === '1.8.0'
        && old.checksum === '6651c9ed80effdc7db0ff72512157f901af5e3549e341e24b1dd4887d836d838'
        && buildEdgeTargets.has(value));
  if (removed.length || newEdges.some(value => !allowed(value))) {
    throw new Error('Unexpected existing dependency edge change: ' + old.name);
  }
  if (newEdges.length) changed.push({name:old.name,added:newEdges});
}
if (JSON.stringify(changed.filter(p => ['rtao','xtask'].includes(p.name)).map(p => p.name).sort()) !== JSON.stringify(['rtao', 'xtask'])) {
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
if (changed.some(p => p.name === 'cc')) {
  const cc = [...after.values()].find(p => p.name === 'cc' && p.version === '1.8.0');
  if (!cc || !closure.has(identity(cc))) throw new Error('Compiler feature edge is outside Docify closure.');
}
if (readFileSync('fuzz/Cargo.lock', 'utf8') !== fuzz
    || git(['diff', '--name-only']).trim() !== 'Cargo.lock') {
  throw new Error('Only the root lock may change during documentation preparation.');
}
console.log(JSON.stringify({changed, newPackages:added.length, existingIdentitiesPreserved:before.size,
  claim:'Candidate only; exact source/lock review, graph regeneration and full hosted supply/tests remain required.'}));
