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
const allowedNames = new Set([
  'docify', 'docify_macros', 'common-path', 'git2', 'libgit2-sys',
  'libssh2-sys', 'libz-sys', 'cmake', 'openssl-probe', 'toml', 'toml_datetime',
  'toml_edit', 'toml_parser', 'toml_writer', 'serde_spanned',
]);
const added = [...after].filter(([identity]) => !before.has(identity));
console.log(JSON.stringify({candidateNewPackages: added.map(([, p]) => ({name:p.name,version:p.version,source:p.source,checksum:p.checksum}))}));
if (added.length < 3 || added.length > 16) throw new Error('Unexpected Docify package count.');
for (const [, current] of added) {
  if (!allowedNames.has(current.name)
      || current.source !== 'registry+https://github.com/rust-lang/crates.io-index'
      || !/^[a-f0-9]{64}$/.test(current.checksum)) {
    throw new Error('Unreviewed new documentation package: ' + current.name);
  }
  if (['docify', 'docify_macros'].includes(current.name) && current.version !== '0.4.1') {
    throw new Error('Docify must stay exactly at 0.4.1.');
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
  if (removed.length || newEdges.some(value => !['rtao', 'xtask'].includes(old.name) || value !== docifyIdentity)) {
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
