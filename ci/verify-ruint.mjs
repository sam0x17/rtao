// Verify the maintained fixed dependency without downloading or compiling it.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const directory = path.join(root, 'patches/ruint');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const provenance = fs.readFileSync(path.join(directory, 'UPSTREAM.json'));
assert.equal(digest(provenance), 'c56928d601aab783c068b741d6c3f715278028e1626f164b68302fd02bfc8685');
const record = JSON.parse(provenance);
assert.equal(record.crate, 'ruint');
assert.equal(record.version, '1.20.1');
assert.equal(record.archive.sha256, '2973657b5127d510e230f5c63d2d106af9c8f79393d8b9f4647323e8196bdde5');
const actual = [];
function inspect(relative = '') {
  for (const entry of fs.readdirSync(path.join(directory, relative), { withFileTypes: true })) {
    const name = path.posix.join(relative, entry.name);
    assert(!entry.isSymbolicLink(), `symlink in maintained source: ${name}`);
    if (entry.isDirectory()) inspect(name);
    else {
      assert(entry.isFile(), `nonregular maintained source: ${name}`);
      if (name !== 'UPSTREAM.json') actual.push(name);
    }
  }
}
inspect();
assert.deepEqual(actual.sort(), Object.keys(record.files).sort());
for (const [name, expected] of Object.entries(record.files)) {
  const bytes = fs.readFileSync(path.join(directory, name));
  assert.equal(bytes.length, expected.bytes, `size: ${name}`);
  assert.equal(digest(bytes), expected.sha256, `hash: ${name}`);
  if (!['Cargo.toml', 'src/bytes.rs'].includes(name)) {
    assert.equal(expected.sha256, expected.upstream_sha256, `unexpected upstream delta: ${name}`);
  }
}
const manifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
assert(manifest.includes('ruint = { path = "patches/ruint" }'), 'workspace must use the reviewed source');
const locks = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8').split('[[package]]');
const identities = locks.filter(entry => /^\s*name = "ruint"$/m.test(entry));
assert.equal(identities.length, 1, 'exactly one fixed integer implementation');
assert(/^version = "1\.20\.1"$/m.test(identities[0]));
assert(!/^source = /m.test(identities[0]), 'fixed integer must resolve to the reviewed path patch');
console.log(`Verified ${actual.length} maintained ruint files and the fixed workspace identity.`);
