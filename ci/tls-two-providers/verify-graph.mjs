// Hosted-only proof of the isolated feature graph. No raw metadata is retained.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

assert.equal(process.env.GITHUB_ACTIONS, 'true', 'this probe runs only in hosted CI');
const phase = process.argv[2];
assert(['prepare-lock', 'check'].includes(phase), 'choose the exact probe phase');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const fixture = path.join(root, 'ci/tls-two-providers');
const outputRoot = path.resolve(process.env.RTAO_TEMP_ROOT || '');
assert.equal(outputRoot, path.join(root, 'target/pr-gates'), 'use the existing hosted task root');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const lock = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
const entries = lock.split('[[package]]').slice(1);
const field = (text, name) => {
  const match = text.match(new RegExp('^' + name + ' = (".*")$', 'm'));
  return match ? JSON.parse(match[1]) : null;
};
const shippedRustls = entries.filter(entry => field(entry, 'name') === 'rustls');
assert.equal(shippedRustls.length, 1, 'one shipped rustls identity');
for (const name of ['aws-lc-rs', 'aws-lc-sys']) {
  assert(!entries.some(entry => field(entry, 'name') === name), 'shipped lock must not contain ' + name);
}
const workspace = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const manifest = fs.readFileSync(path.join(fixture, 'Cargo.toml'), 'utf8');
const start = workspace.indexOf('\n[patch.crates-io]\n');
const fixtureStart = manifest.indexOf('\n[patch.crates-io]\n');
assert(start >= 0 && fixtureStart >= 0, 'reviewed root patches must be retained');
assert.equal(manifest.slice(fixtureStart), workspace.slice(start).replace(
  'ruint = { path = "patches/ruint" }', 'ruint = { path = "../../patches/ruint" }',
), 'the fixture must carry the same core/BLS/fixed-integer patches');
const metadata = JSON.parse(execFileSync('cargo', [
  'metadata', '--locked', '--format-version', '1', '--manifest-path', path.join(fixture, 'Cargo.toml'),
], { cwd: root, encoding: 'utf8', timeout: 180000, maxBuffer: 16 * 1024 * 1024 }));
assert.equal(path.resolve(metadata.workspace_root), fixture, 'independent workspace root');
const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
const active = metadata.packages.filter(pkg => nodes.has(pkg.id));
const exactly = name => {
  const found = active.filter(pkg => pkg.name === name);
  assert.equal(found.length, 1, 'one resolved ' + name);
  return found[0];
};
const probe = exactly('rtao-tls-two-providers');
assert.deepEqual(metadata.workspace_members, [probe.id], 'only the probe belongs to this workspace');
const rustls = exactly('rustls');
assert.equal(rustls.version, field(shippedRustls[0], 'version'), 'exercise the shipped rustls version');
for (const feature of ['ring', 'aws_lc_rs']) {
  assert(nodes.get(rustls.id).features.includes(feature), 'missing rustls feature ' + feature);
}
for (const name of ['rtao-tls-two-providers', 'rtao-rpc', 'rtao-testkit']) {
  const pkg = exactly(name);
  const direct = nodes.get(pkg.id).deps.filter(dep => dep.name === 'rustls');
  assert.equal(direct.length, 1, name + ' must have one direct rustls dependency');
  assert.equal(direct[0].pkg, rustls.id, name + ' must use the same two-provider rustls');
}
const byId = new Map(metadata.packages.map(pkg => [pkg.id, pkg]));
const dependency = (parent, name) => {
  const found = nodes.get(parent.id).deps.map(dep => byId.get(dep.pkg)).filter(pkg => pkg.name === name);
  assert.equal(found.length, 1, parent.name + ' must resolve one ' + name);
  return found[0];
};
const aws = dependency(rustls, 'aws-lc-rs');
const providers = [dependency(rustls, 'ring'), aws, dependency(aws, 'aws-lc-sys')]
  .map(pkg => ({ name: pkg.name, version: pkg.version, source: pkg.source }));
execFileSync('git', ['diff', '--exit-code', '--', 'Cargo.toml', 'Cargo.lock', 'fuzz/Cargo.lock'], { cwd: root });
const sha = process.env.GITHUB_SHA;
assert(/^[a-f0-9]{40}$/.test(sha || ''), 'exact source commit required');
const inputs = ['ci/tls-two-providers/Cargo.toml', 'ci/tls-two-providers/Cargo.lock',
  'ci/tls-two-providers/tests/two_providers.rs', 'ci/tls-two-providers/verify-graph.mjs',
  'Cargo.toml', 'Cargo.lock', 'crates/rtao-rpc/src/session.rs', 'crates/rtao-testkit/src/fakes/node.rs'];
const hashes = Object.fromEntries(inputs.map(name => [name, digest(fs.readFileSync(path.join(root, name)))]));
const proof = { schema: 1, phase, head: sha, workspace: 'ci/tls-two-providers',
  rustls: { version: rustls.version, features: [...nodes.get(rustls.id).features].sort() },
  providers, hashes, rootLockUnchanged: true,
  scope: 'Resolved graph only. The named CI test step separately establishes compiled-provider TLS execution.' };
const output = JSON.stringify(proof, null, 2) + '\n';
assert(Buffer.byteLength(output) < 16384, 'bounded graph proof');
fs.mkdirSync(path.join(outputRoot, 'tls-two-providers'), { recursive: true });
fs.writeFileSync(path.join(outputRoot, 'tls-two-providers/graph.json'), output);
console.log('Verified isolated ' + rustls.version + ' ring/AWS-LC graph at ' + sha + '.');
