require 'digest'
require 'fileutils'
require 'find'
require 'json'
require 'open3'

root = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC'
evidence = '/Users/sam/workspace/rtao/docs/upstream/validation/quality-unicode-publication'
mode = ARGV.fetch(0)
allowed = {
  'probes' => [
    "#{root}/upstream-quality/pr-3246/sdk/bittensor-core/tests/quality_fee_audit.rs",
    "#{root}/upstream-quality/pr-3247/sdk/bittensor-core/tests/quality_client_audit.rs",
    "#{root}/upstream-quality/pr-3247/sdk/bittensor-core/tests/quality"
  ],
  'finished' => [
    "#{root}/upstream-quality/pr-3246",
    "#{root}/upstream-quality/pr-3247",
    "#{root}/upstream-quality/unicode-publication"
  ]
}
targets = allowed.fetch(mode)
targets.each do |path|
  abort "unexpected cleanup path #{path}" unless File.exist?(path) && !File.symlink?(path) && File.realpath(path) == path
end
FileUtils.mkdir_p(evidence)
report = { mode: mode, paths: targets, dependencies: [], errors: [], removed: false }

if mode == 'probes'
  archive = "#{root}/tmp/quality-protocol-probes.tar.gz"
  abort 'probe archive hash mismatch' unless Digest::SHA256.file(archive).hexdigest == '7b7c6b446a7496b9243924d66f783ac48865baa53f14959d2348d0b031ebe3f6'
  report[:archive] = { path: archive, sha256: Digest::SHA256.file(archive).hexdigest }
  files = targets.flat_map { |path| File.directory?(path) ? Dir.glob("#{path}/**/*").select { |entry| File.file?(entry) } : [path] }
  report[:archived_files] = files.map do |path|
    member = path.delete_prefix("#{root}/upstream-quality/")
    bytes, err, status = Open3.capture3('tar', '-xOf', archive, member)
    abort "archive read failed #{err}" unless status.success? && bytes.b == File.binread(path)
    { path: path, member: member, sha256: Digest::SHA256.file(path).hexdigest }
  end
end

ancestors = [Process.pid]
pid = Process.ppid
while pid > 1
  ancestors << pid
  out, status = Open3.capture2('ps', '-p', pid.to_s, '-o', 'ppid=')
  break unless status.success?
  pid = out.to_i
end
processes, err, status = Open3.capture3('ps', '-axo', 'pid=,ppid=,command=')
abort "process audit failed #{err}" unless status.success?
processes.each_line do |line|
  next if ancestors.include?(line.split.first.to_i)
  report[:dependencies] << { kind: 'process', line: line.strip } if targets.any? { |path| line.include?(path) }
end
report[:launchd_domains] = ["gui/#{Process.uid}", 'system'].map do |domain|
  out, err, status = Open3.capture3('launchctl', 'print', domain)
  report[:errors] << { kind: 'launchd', domain: domain, error: err } unless status.success?
  hits = out.lines.select { |line| targets.any? { |path| line.include?(path) } }
  report[:dependencies] << { kind: 'launchd', domain: domain, lines: hits } unless hits.empty?
  { domain: domain, success: status.success? }
end
config_roots = [
  '/Users/sam/Library/LaunchAgents', '/Library/LaunchAgents', '/Library/LaunchDaemons',
  '/Users/sam/.config', '/Users/sam/.cargo/config.toml', '/Users/sam/.gitconfig',
  '/Users/sam/.ssh/config', '/Users/sam/.zshrc', '/Users/sam/.zprofile',
  '/Users/sam/.bashrc', '/Users/sam/.bash_profile', '/Users/sam/workspace/rtao/.cargo',
  '/Users/sam/workspace/rtao/.config', '/Users/sam/workspace/rtao/Cargo.toml',
  '/Users/sam/workspace/rtao/Cargo.lock', '/Users/sam/workspace/rtao/.git/config'
].select { |path| File.exist?(path) }
targets.each do |path|
  out, err, status = Open3.capture3('rg', '-l', '-F', '--hidden', path, *config_roots)
  report[:errors] << { kind: 'configuration', error: err } unless [0, 1].include?(status.exitstatus)
  report[:dependencies] << { kind: 'configuration', files: out.lines.map(&:strip) } unless out.empty?
  args = File.directory?(path) ? ['lsof', '-w', '+D', path] : ['lsof', '-w', path]
  out, err, status = Open3.capture3(*args)
  report[:errors] << { kind: 'open-files', error: err } unless [0, 1].include?(status.exitstatus)
  report[:dependencies] << { kind: 'open-files', output: out } unless out.empty?
end
report[:configuration_roots] = config_roots
scan_roots = ['/Users/sam/workspace', '/Users/sam/.config', '/Users/sam/.local', '/Users/sam/.venvs', '/usr/local/bin', '/opt/homebrew/bin'].select { |path| File.directory?(path) }
out, err, status = Open3.capture3('find', *scan_roots, '(', '-type', 'l', '-o', '-name', 'pyvenv.cfg', '-o', '-name', '*.pth', '-o', '-name', '*.egg-link', '-o', '-path', '*/.git/config', ')', '-print0')
report[:errors] << { kind: 'dependency-scan', error: err } unless status.success?
entries = out.split("\0")
report[:dependency_entries_checked] = entries.length
entries.each do |path|
  next if targets.any? { |target| path == target || path.start_with?(target + '/') }
  if File.symlink?(path)
    destination = File.expand_path(File.readlink(path), File.dirname(path))
    report[:dependencies] << { kind: 'symlink', path: path, target: destination } if targets.any? { |target| destination == target || destination.start_with?(target + '/') }
  elsif File.file?(path)
    bytes = File.binread(path)
    kind = path.end_with?('/.git/config') ? 'git-configuration' : 'virtual-environment'
    report[:dependencies] << { kind: kind, path: path } if targets.any? { |target| bytes.include?(target) }
  end
end
report[:scan_roots] = scan_roots
report[:bytes] = targets.sum do |target|
  total = 0
  Find.find(target) { |path| total += File.size(path) if File.file?(path) && !File.symlink?(path) }
  total
end
output = "#{evidence}/cleanup-#{mode}.json"
File.write(output, JSON.pretty_generate(report) + "\n")
abort 'cleanup dependency or audit error found; nothing removed' unless report[:dependencies].empty? && report[:errors].empty?
if mode == 'finished'
  report[:heads] = [3246, 3247].map do |id|
    path = "#{root}/upstream-quality/pr-#{id}"
    out, status = Open3.capture2('git', '-C', path, 'status', '--porcelain')
    abort "dirty finished checkout #{path}" unless status.success? && out.empty?
    sha, status = Open3.capture2('git', '-C', path, 'rev-parse', 'HEAD')
    abort 'head read failed' unless status.success?
    expected = File.read("#{evidence}/pr-#{id}-head.txt").strip
    abort "head mismatch #{path}" unless sha.strip == expected
    out, err, status = Open3.capture3('git', '-C', "#{root}/upstream-quality/subtensor.git", 'worktree', 'remove', path)
    abort "worktree removal failed #{out} #{err}" unless status.success?
    { path: path, head: sha.strip }
  end
  FileUtils.rm_rf("#{root}/upstream-quality/unicode-publication")
else
  targets.each { |path| FileUtils.rm_rf(path) }
end
report[:removed] = true
report[:retained] = 'Shared task build/dependency caches, bare repository, other review worktrees, original wrappers and authenticated Unicode audit sources/proof remain live and preserved.'
File.write(output, JSON.pretty_generate(report) + "\n")
puts JSON.pretty_generate(report)
