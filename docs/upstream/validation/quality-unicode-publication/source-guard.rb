require 'digest'
require 'json'
require 'open3'

pr = ARGV.fetch(0)
original = {
  '3246' => '9e926d1f2f4943f11a23ef13ff517e7857e9cc27',
  '3247' => 'aadeb06bb517b24428f4d7f64a351ccc8765e502'
}.fetch(pr)
root = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC'
checkout = "#{root}/upstream-quality/pr-#{pr}"
evidence = File.expand_path(File.dirname(__FILE__))
Dir.chdir(checkout)
def git(*args)
  out, err, status = Open3.capture3('git', *args)
  abort "git guard failed #{err}" unless status.success?
  out
end
head = git('rev-parse', 'HEAD').strip
abort 'dirty source' unless git('status', '--porcelain').empty?
abort 'head changed' unless head == File.read("#{evidence}/pr-#{pr}-head.txt").strip
git('merge-base', '--is-ancestor', original, head)
abort 'original head unchanged' if head == original
expected_paths = %w[
  Cargo.lock
  sdk/bittensor-core/Cargo.toml
  sdk/bittensor-core/src/protocol/dispatch_error.rs
  sdk/bittensor-core/src/protocol/python_printable.rs
  sdk/bittensor-core/tests/portable_fee_dispatch.rs
]
abort 'unexpected correction paths' unless git('diff', '--name-only', original, head).lines.map(&:strip).sort == expected_paths.sort
git('diff', '--check', original, head)
base = '850f2755bb05a3851f948489f0ab27b0645cebce'
full_paths = git('diff', '--name-only', base, head).lines.map(&:strip)
abort 'unexpected full stacked scope' unless full_paths.all? { |path| path == 'Cargo.lock' || path.start_with?('sdk/bittensor-core/') }
git('diff', '--exit-code', base, head, '--', 'Cargo.lock')
original_lock = git('show', "#{original}:Cargo.lock")
edge = " \"unicode-general-category\",\n"
stanza = "[[package]]\nname = \"unicode-general-category\"\nversion = \"1.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"0b993bddc193ae5bd0d623b49ec06ac3e9312875fdae725a975c51db1cc1677f\"\n\n"
abort 'unexpected original lock edge/stanza' unless original_lock.scan(edge).length == 1 && original_lock.scan(stanza).length == 1
expected_lock = original_lock.sub(edge, '').sub(stanza, '')
abort 'lock differs beyond Unicode removal' unless File.binread('Cargo.lock') == expected_lock
manifest = git('show', "#{original}:sdk/bittensor-core/Cargo.toml")
expected_manifest = manifest.sub("license = \"Apache-2.0\"", "license = \"Apache-2.0 AND Unicode-DFS-2016\"").sub("unicode-general-category = \"1\"\n", '')
abort 'unexpected manifest correction' unless File.read('sdk/bittensor-core/Cargo.toml') == expected_manifest
reviewed = JSON.parse(File.read("#{root}/upstream-quality/unicode-audit/patch-manifest.json")).select { |entry| entry['lane'] == 'staged-upstream' }
reviewed.each do |entry|
  next if entry['path'] == 'sdk/bittensor-core/Cargo.toml'
  actual = Digest::SHA256.file(entry.fetch('path')).hexdigest
  abort "reviewed source mismatch #{entry['path']}" unless actual == entry.fetch('corrected_sha256')
end
proof = {
  pr: pr.to_i, original: original, head: head, base: base,
  correction_paths: expected_paths, full_stacked_paths: full_paths,
  lock_delta: 'Exactly the Unicode dependency edge and 1.1.0 package removed (7 lines); final lock byte-identical to v478 base.',
  unicode_cases: 21,
  license: 'Apache-2.0 AND Unicode-DFS-2016',
  hashes: expected_paths.to_h { |path| [path, Digest::SHA256.file(path).hexdigest] }
}
File.write("#{evidence}/pr-#{pr}-scope.json", JSON.pretty_generate(proof) + "\n")
puts JSON.generate(proof)
