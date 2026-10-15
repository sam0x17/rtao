require 'digest'
require 'json'

pr = ARGV.fetch(0)
abort 'unexpected PR' unless %w[3246 3247].include?(pr)
dir = File.expand_path(File.dirname(__FILE__))
results = {}
%w[1.89.0 stable].each do |toolchain|
  portable = File.read("#{dir}/pr-#{pr}-#{toolchain}-portable.log")
  counts = portable.scan(/test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/).map { |row| row.map(&:to_i) }
  expected = pr == '3246' ? [13, 5, 9] : [13, 5, 4, 9]
  abort "unexpected #{toolchain} portable results #{counts.inspect}" unless counts.map(&:first).sort == expected.sort && counts.all? { |row| row[1..] == [0, 0] }
  abort 'Unicode regression missing' unless portable.include?('test result_message_uses_cpython312_unicode15 ... ok')
  library = File.read("#{dir}/pr-#{pr}-#{toolchain}-lib.log")
  abort 'library count mismatch' unless library.include?('test result: ok. 71 passed; 0 failed; 1 ignored;')
  abort 'unexpected ignored test' unless library.match?(/test pow::tests::gpu_mines_and_revalidates_chain_compatible_work \.\.\. ignored/)
  results[toolchain] = { portable: counts.sum(&:first), unicode_public_test: 1, unicode_cases: 21, library: 71, physical_OpenCL_GPU_ignored: 1, binding: 'all-feature/all-target compile; no Rust unit tests defined' }
end
stages = %w[fmt 1.89.0-portable 1.89.0-lib 1.89.0-binding stable-portable stable-lib stable-binding clippy-msrv wasm-msrv wasm-current]
logs = stages.to_h do |stage|
  path = "#{dir}/pr-#{pr}-#{stage}.log"
  [File.basename(path), Digest::SHA256.file(path).hexdigest]
end
File.write("#{dir}/pr-#{pr}-results.json", JSON.pretty_generate({pr: pr.to_i, results: results, strict_clippy: '1.89.0, all features/targets, core and binding, separate disposable target, warnings denied', portable_wasm: ['1.89 alias (rustc 1.89.0)', 'stable (rustc 1.99.0)'], log_hashes: logs}) + "\n")
puts JSON.generate(results)
