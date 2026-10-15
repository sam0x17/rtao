require 'json'
require 'open3'
require 'rbconfig'

audit = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/ledger-audit'
source = audit + '/zondax'
prior = '9edd5fa5f9e609046e6bdd0f5e77239a7cf6f20d'
fork = 'git@github.com:sam0x17/ledger-polkadot.git'
branch = 'refs/heads/fix/tao-balance'
allowed = %w[app/src/metadata_parser.c docs/format.md tests/ledger_quality.cpp tests/quality/README.md tests/quality/relative-price-v478.json tests/supported_chains/bittensor-477.json].sort

def checked(*arguments)
  output, error, status = Open3.capture3(*arguments)
  raise "command failed: #{arguments.first}: #{error}" unless status.success?
  output.strip
end

Dir.chdir(source)
raise 'wrong checkout' unless checked('git', 'rev-parse', '--show-toplevel') == source
raise 'unexpected hook mode' unless %w[pre-commit pre-push].include?(ARGV[0])
mode = ARGV.shift
paths = if mode == 'pre-commit'
          raise 'unexpected starting head' unless checked('git', 'rev-parse', 'HEAD') == prior
          checked('git', 'diff', '--cached', '--name-only').split("\n").sort
        else
          raise 'wrong push destination' unless ARGV == ['fork', fork]
          raise 'wrong fork config' unless checked('git', 'remote', 'get-url', '--push', 'fork') == fork
          raise 'dirty committed checkout' unless checked('git', 'status', '--porcelain').empty?
          head = checked('git', 'rev-parse', 'HEAD')
          raise 'unexpected parent' unless checked('git', 'rev-parse', 'HEAD^') == prior
          changes = STDIN.read.lines.map(&:split)
          raise 'unexpected ref update' unless changes == [[branch, head, branch, prior]]
          raise 'wrong GitHub actor' unless checked('gh', 'api', 'user', '--jq', '.login') == 'sam0x17'
          checked('git', 'diff', '--name-only', prior, 'HEAD').split("\n").sort
        end
raise 'unexpected changed files' unless paths == allowed

checked('git', 'diff', '--check')
checked('git', 'diff', '--cached', '--check')
checked(audit + '/native-tools/clang-format-21', '--dry-run', '--Werror', 'app/src/metadata_parser.c', 'tests/ledger_quality.cpp')

if mode == 'pre-push'
  checked('git', 'diff', '--check', prior, 'HEAD')
  environment = {'TMPDIR' => '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp'}
  File.open(audit + '/publication-committed-build.log', 'w') do |log|
    raise 'committed native build failed' unless system(environment, RbConfig.ruby, audit + '/native-cpp-build.rb', out: log, err: log)
  end
  File.open(audit + '/publication-committed-suite.log', 'w') do |log|
    raise 'committed native suite failed' unless system(environment, audit + '/cpp-pr/unittests', out: log, err: log)
  end
  suite = File.read(audit + '/publication-committed-suite.log')
  raise 'unexpected native test inventory' unless suite.include?('[  PASSED  ] 5604 tests.')
  File.write(audit + '/publication-gate-result.json', JSON.pretty_generate({head: checked('git', 'rev-parse', 'HEAD'), parent: prior, actor: 'sam0x17', remote: fork, branch: branch, files: paths, passed: 5604, local_python: false, cmake_or_hunter: false, clang_format: checked(audit + '/native-tools/clang-format-21', '--version')}))
  puts 'Committed-head gate passed: 5604 native tests, formatting, exact fork/branch/file scope.'
end
