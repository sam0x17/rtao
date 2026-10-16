// A hosted-only migration candidate at the existing parity pin. Never imports
// ordinary upstream files, changes pins or publishes the candidate.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';

const commit = '850f2755bb05a3851f948489f0ab27b0645cebce';
const upstream = 'https://github.com/RaoFoundation/subtensor.git';
const canonicalRoot = 'precompiles/src/solidity/';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const fail = message => { throw new Error(message); };
const run = (args, cwd = process.cwd()) => execFileSync('git', args, {
  cwd, encoding: 'utf8', maxBuffer: 2 * 1024 * 1024, timeout: 90_000,
  env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1' },
});
const regular = (path, maxBytes = 2 * 1024 * 1024) => {
  const info = lstatSync(path);
  if (!info.isFile() || info.isSymbolicLink() || info.size > maxBytes) fail(`Expected bounded regular file: ${path}`);
  return readFileSync(path);
};

if (process.env.GITHUB_ACTIONS !== 'true'
    || process.env.RUNNER_ENVIRONMENT !== 'github-hosted'
    || process.env.RUNNER_OS !== 'Linux' || process.env.RUNNER_ARCH !== 'X64'
    || process.env.GITHUB_EVENT_NAME !== 'workflow_dispatch') {
  fail('Canonical ABI preparation is restricted to hosted Ubuntu dispatches.');
}
const root = realpathSync(process.cwd());
if (root !== realpathSync(process.env.GITHUB_WORKSPACE ?? '.')) fail('Run from the workflow checkout.');
if (!process.env.RTAO_TEMP_ROOT
    || resolve(process.env.RTAO_TEMP_ROOT) !== join(root, 'target/pr-gates')) {
  fail('Canonical ABI work requires the recorded hosted temporary root.');
}
const taskRoot = join(root, 'target/pr-gates/canonical-abis');
for (let path = dirname(taskRoot); path !== root; path = dirname(path)) {
  if (existsSync(path) && (!lstatSync(path).isDirectory() || lstatSync(path).isSymbolicLink())) {
    fail(`Unsafe hosted temporary ancestor: ${path}`);
  }
}
const cache = join(taskRoot, 'upstream.git');
const candidate = join(taskRoot, 'candidate');
const head = run(['rev-parse', 'HEAD']).trim();
if (!/^[0-9a-f]{40}$/.test(head) || head !== process.env.GITHUB_SHA) fail('Checkout differs from the selected source commit.');
const baseline = run(['show', 'HEAD:upstream.toml']);
if (!baseline.includes(`[parity]\ntag = "v478"\ncommit = "${commit}"\n`)
    || !baseline.includes(`[monorepo]\nurl = "${upstream}"\n`)) {
  fail('The reviewed parity pin or upstream identity changed.');
}
const sdkRows = [...baseline.matchAll(/^"(sdk\/python\/bittensor\/evm\/abi\/([A-Za-z0-9]+)\.json)" = \{ to = "([^"]+)", sha256 = "([0-9a-f]{64})" \}$/gm)];
if (sdkRows.length !== 21 || new Set(sdkRows.map(row => row[2])).size !== 21) fail('Expected exactly 21 distinct pinned SDK ABIs.');
const sources = sdkRows.map(row => ({
  sdk: row[1], stem: row[2], sdkTo: row[3], sdkSha256: row[4],
  source: `${canonicalRoot}${row[2]}.abi`,
  to: `vendor/upstream/src/${canonicalRoot}${row[2]}.abi`,
}));
if (sources.some(row => row.sdkTo !== `vendor/upstream/evm/abi/${row.stem}.json`)
    || baseline.includes(`"${canonicalRoot}`)) fail('Candidate requires the original SDK-only baseline.');

const mode = process.argv[2];
if (process.argv.length !== 3 || !['prepare', 'verify'].includes(mode)) fail('Use prepare or verify.');
if (mode === 'prepare') {
  if (run(['status', '--porcelain', '-z', '--untracked-files=all']) !== '') fail('Candidate requires a clean source checkout.');
  if (existsSync(taskRoot)) fail('Canonical ABI task root already exists; do not overwrite another task.');
  mkdirSync(taskRoot, { recursive: true, mode: 0o700 });
  run(['init', '--bare', cache]);
  run(['remote', 'add', 'origin', upstream], cache);
  run(['config', 'remote.origin.promisor', 'true'], cache);
  run(['config', 'remote.origin.partialclonefilter', 'blob:none'], cache);
  run(['fetch', '--depth=1', '--filter=blob:none', '--no-tags', 'origin', commit], cache);
  if (run(['rev-parse', 'FETCH_HEAD'], cache).trim() !== commit) fail('Fetched source differs from the exact parity pin.');
  console.log(`Prepared exact-pin canonical ABI cache under ${relative(root, taskRoot)}.`);
} else {
  if (realpathSync(taskRoot) !== taskRoot || realpathSync(cache) !== cache
      || run(['rev-parse', '--is-bare-repository'], cache) !== 'true\n'
      || run(['remote', 'get-url', 'origin'], cache).trim() !== upstream
      || run(['config', '--bool', 'remote.origin.promisor'], cache) !== 'true\n'
      || run(['config', 'remote.origin.partialclonefilter'], cache) !== 'blob:none\n'
      || run(['rev-parse', 'FETCH_HEAD'], cache).trim() !== commit) fail('Canonical source cache identity changed.');

  const manifest = regular('upstream.toml').toString('utf8');
  const notice = regular('NOTICE').toString('utf8');
  const oldNotice = run(['show', 'HEAD:NOTICE']);
  const proofs = [];
  let retainedManifest = manifest;
  let retainedNotice = notice;
  let total = 0;
  for (const row of sources) {
    const bytes = regular(row.to, 64 * 1024);
    total += bytes.length;
    if (bytes.length > 64 * 1024 || total > 1024 * 1024) fail('Canonical candidate exceeds the reviewed size bounds.');
    const digest = sha256(bytes);
    const blob = createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
    const tree = run(['ls-tree', '-r', '--full-tree', commit, '--', row.source, row.sdk], cache).trim().split('\n');
    if (tree.length !== 2 || tree.some(line => !line.startsWith(`100644 blob ${blob}\t`))
        || !tree.some(line => line.endsWith(`\t${row.source}`))
        || !tree.some(line => line.endsWith(`\t${row.sdk}`))) fail(`Exact-pin source/blob identity differs: ${row.source}`);
    if (digest !== row.sdkSha256 || !regular(row.sdkTo).equals(bytes)) fail(`SDK/canonical byte identity differs: ${row.source}`);
    const manifestLine = `"${row.source}" = { to = "${row.to}", sha256 = "${digest}" }\n`;
    const noticeLine = `${row.source} -> ${row.to}; ${digest}\n`;
    if (retainedManifest.split(manifestLine).length !== 2 || retainedNotice.split(noticeLine).length !== 2) fail(`Missing or repeated generated attribution: ${row.source}`);
    retainedManifest = retainedManifest.replace(manifestLine, '');
    retainedNotice = retainedNotice.replace(noticeLine, '');
    proofs.push({ source: row.source, sdk_source: row.sdk, to: row.to, bytes: bytes.length, git_blob: blob, sha256: digest });
  }
  if (retainedManifest !== baseline || retainedNotice !== oldNotice) fail('Candidate changed pins, existing artifact rows or existing attribution.');
  const expected = new Set(['upstream.toml', 'NOTICE', ...sources.map(row => row.to)]);
  const changed = run(['status', '--porcelain', '-z', '--untracked-files=all']).split('\0').filter(Boolean);
  if (changed.length !== expected.size || changed.some(line =>
    ![' M', '??'].includes(line.slice(0, 2)) || !expected.delete(line.slice(3))) || expected.size) {
    fail('Candidate changed unexpected paths or failed to generate every expected path.');
  }
  const testProofs = [];
  for (const filter of ['import_upstream::tests::', 'extract::resolved::precompiles::tests::']) {
    const log = regular(join(taskRoot, `${filter.startsWith('import') ? 'import' : 'extract'}-tests.log`), 1024 * 1024).toString('utf8');
    const names = [...log.matchAll(/^test ([^\n]+) \.\.\. ok$/gm)].map(match => match[1]);
    const summary = [...log.matchAll(/^test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; (\d+) filtered out;[^\n]*$/gm)];
    if (summary.length !== 1 || Number(summary[0][1]) < 1 || names.length !== Number(summary[0][1])
        || names.some(name => !name.startsWith(filter)) || new Set(names).size !== names.length) fail(`Focused canonical tests were missing or incomplete: ${filter}`);
    const required = filter.startsWith('import') ? [
      'canonical_abis_import_preserves_existing_pins_and_artifacts',
      'canonical_abis_import_refuses_wrong_pin_cache_inventory_and_existing_changes',
      'canonical_abis_import_refuses_drift_before_publishing',
    ] : [
      'all_imported_sdk_abis_match_independent_canonical_solidity_arrays',
      'canonical_abi_input_and_artifact_pin_are_both_required',
      'reauthenticated_canonical_abi_content_drift_and_non_arrays_are_refused',
      'canonical_abi_json_formatting_and_object_key_order_do_not_change_equality',
      'unregistered_watched_sdk_json_requires_its_own_canonical_counterpart',
      'sdk_abi_hash_blob_and_destination_authentication_survive_canonical_checks',
    ];
    for (const name of required) {
      if (!names.includes(filter + name)) fail(`Required canonical fixture did not pass: ${filter}${name}`);
    }
    testProofs.push({ filter, passed: names.length, tests: names });
  }
  if (existsSync(candidate)) fail('Candidate staging directory already exists.');
  mkdirSync(candidate, { mode: 0o700 });
  for (const path of ['upstream.toml', 'NOTICE', ...sources.map(row => row.to)]) {
    const output = join(candidate, path);
    mkdirSync(dirname(output), { recursive: true });
    copyFileSync(path, output);
  }
  const proof = {
    schema: 1, source_commit: head, upstream, parity_commit: commit,
    run_id: process.env.GITHUB_RUN_ID, run_attempt: process.env.GITHUB_RUN_ATTEMPT,
    upstream_manifest_sha256: sha256(Buffer.from(manifest)), notice_sha256: sha256(Buffer.from(notice)),
    canonical_bytes: total, canonical_files: proofs, focused_tests: testProofs,
    preserved: 'All noncanonical manifest bytes, existing NOTICE entries and other checkout paths are unchanged.',
    limitation: 'Hosted import/extraction and focused fixtures only; ordinary full gates and candidate adoption remain separate.',
  };
  writeFileSync(join(candidate, 'canonical-source-proof.json'), `${JSON.stringify(proof, null, 2)}\n`);
  console.log(`Verified and staged ${proofs.length} exact-pin ABI candidates (${total} bytes).`);
}
