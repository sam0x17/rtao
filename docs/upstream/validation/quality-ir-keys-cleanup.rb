require 'find'
require 'json'
require 'open3'
require 'digest'
require 'fileutils'

task_root = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC'
bare = File.join(task_root, 'upstream-quality/subtensor.git')
checkouts = {
  File.join(task_root, 'upstream-quality/pr-3249') => '0c4ed7585ae9d232577e53d45813f383e0ec2eec',
  File.join(task_root, 'upstream-quality/pr-3250') => '64f152725dda64c040e8f3c6435c08f450093a59',
}
lint_paths = %w[ir-keys-u9 ir-keys-u10].map { |name| File.join(task_root, 'upstream-quality/lint', name) }
owned_paths = checkouts.keys + lint_paths
expected_lock = 'c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a'
docs = '/Users/sam/workspace/rtao/docs/upstream/validation'
raise 'published evidence missing' unless File.read(File.join(docs, 'quality-ir-keys.md')).include?('Remote branch and PR HEAD were confirmed')
owned_paths.each do |path|
  raise "invalid exact directory: #{path}" unless File.directory?(path) && !File.symlink?(path) && File.realpath(path) == path
end
checkouts.each do |path, expected_head|
  head, head_status = Open3.capture2('git', '-C', path, 'rev-parse', 'HEAD')
  raise 'wrong checkout HEAD' unless head_status.success? && head.strip == expected_head
  diff, diff_status = Open3.capture2('git', '-C', path, 'diff', '--exit-code', 'HEAD')
  raise 'changed tracked source' unless diff_status.success? && diff.empty?
  raise 'changed lock' unless Digest::SHA256.file(File.join(path, 'Cargo.lock')).hexdigest == expected_lock
  status, status_code = Open3.capture2('git', '-C', path, 'status', '--short', '--untracked-files=all')
  raise 'status query failed' unless status_code.success?
  expected_untracked = path.end_with?('/pr-3249') ? "?? sdk/bittensor-core/tests/review_ir_independent.rs\n" : ''
  raise 'unrelated untracked content' unless status == expected_untracked
end
raise 'independent metadata source not retained' unless Digest::SHA256.file(File.join(checkouts.keys.first, 'sdk/bittensor-core/tests/review_ir_independent.rs')).hexdigest == Digest::SHA256.file(File.join(docs, 'quality-ir-independent.rs')).hexdigest

ancestor_ids = [Process.pid]
parent_id = Process.ppid
while parent_id > 1
  ancestor_ids << parent_id
  result, status = Open3.capture2('ps', '-p', parent_id.to_s, '-o', 'ppid=')
  break unless status.success?
  parent_id = result.to_i
end
ps, ps_status = Open3.capture2('ps', '-axo', 'pid=,args=')
raise 'process query failed' unless ps_status.success?
process_matches = ps.lines.select { |line| !ancestor_ids.include?(line.split.first.to_i) && owned_paths.any? { |path| line.include?(path) } }
raise "live process count: #{process_matches.size}" unless process_matches.empty?
lsof_limits = []
owned_paths.each do |path|
  stdout, stderr, status = Open3.capture3('lsof', '-nP', '+D', path)
  raise "open file or lsof error: #{path}" unless stdout.empty? && status.exitstatus == 1
  lsof_limits << stderr unless stderr.empty?
end
launchd = {}
["gui/#{Process.uid}", 'system'].each do |domain|
  stdout, stderr, status = Open3.capture3('launchctl', 'print', domain)
  raise "launchd query failed: #{domain}" unless status.success?
  launchd[domain] = stdout.lines.count { |line| owned_paths.any? { |path| line.include?(path) } }
  raise 'live launchd reference' unless launchd[domain].zero?
end

refs = []
errors = []
links = 0
dependency_files = 0
removed_counts = owned_paths.to_h { |path| [path, {files: 0, bytes: 0, directories: 0, symlinks: []}] }
scan_roots = [task_root, '/Users/sam/.config', '/Users/sam/Library/LaunchAgents', '/Library/LaunchDaemons', '/Library/LaunchAgents', '/Users/sam/workspace/rtao/.cargo']
scan_roots.select { |path| File.exist?(path) }.each do |scan_root|
  Find.find(scan_root) do |path|
    begin
      stat = File.lstat(path)
      owner = owned_paths.find { |owned| path == owned || path.start_with?(owned + '/') }
      if stat.symlink?
        links += 1
        target = File.expand_path(File.readlink(path), File.dirname(path))
        target = File.realpath(target) if File.exist?(target)
        if owner
          removed_counts[owner][:symlinks] << {path: path, target: target}
        elsif owned_paths.any? { |owned| target == owned || target.start_with?(owned + '/') }
          refs << {path: path, kind: 'inbound symlink', target: target}
        end
        next
      end
      if stat.directory?
        removed_counts[owner][:directories] += 1 if owner
        Find.prune if path == File.join(task_root, 'build') || path.end_with?('/objects') && path.include?('.git')
        next
      end
      next unless stat.file?
      if owner
        removed_counts[owner][:files] += 1
        removed_counts[owner][:bytes] += stat.size
      end
      name = File.basename(path)
      candidate = %w[pyvenv.cfg .git config config.toml].include?(name) || name.end_with?('.pth', '.egg-link', '.plist') || name.start_with?('activate')
      next unless candidate && stat.size < 1_048_576
      dependency_files += 1
      if !owner
        contents = File.binread(path)
        refs << {path: path, kind: 'configuration/environment/Git reference'} if owned_paths.any? { |owned| contents.include?(owned) }
      end
    rescue StandardError => e
      errors << {path: path, error: e.class.name}
    end
  end
end
['/Users/sam/.codex/config.toml', '/Users/sam/.cargo/config.toml', '/Users/sam/.zshrc', '/Users/sam/.zprofile', *Dir.glob('/Users/sam/.cargo/git/**/config')].select { |path| File.file?(path) }.each do |path|
  dependency_files += 1
  contents = File.binread(path)
  refs << {path: path, kind: 'user/Cargo Git configuration'} if owned_paths.any? { |owned| contents.include?(owned) }
end
raise "dependency references or scan errors: #{refs.size}/#{errors.size}" unless refs.empty? && errors.empty?

report = {
  owned_paths: owned_paths, heads: checkouts, lock_sha256: expected_lock,
  counts: removed_counts, process_matches: 0, open_files: 0,
  launchd_matches: launchd, symlinks_inspected: links,
  configuration_environment_Git_files_inspected: dependency_files,
  dependency_references: refs, scan_errors: errors, lsof_limitations: lsof_limits.uniq,
  retained_live_dependencies: [File.join(task_root, 'build'), bare, File.join(task_root, 'upstream-quality/ir-keys-proof/rustc-u9.sh'), File.join(task_root, 'upstream-quality/ir-keys-proof/rustc-u10.sh'), File.join(task_root, 'upstream-quality/u10-outgoing-64f152725dda64c040e8f3c6435c08f450093a59'), 'Other owners task directories and shared caches'],
  note: 'Compiler wrappers and the shared build/bare/source caches are retained by root request. Dep-info and retained logs are provenance rather than live runtime or virtual-environment dependencies.',
  removed: false,
}
report_path = File.join(task_root, 'tmp/quality-ir-keys-cleanup.json')
File.write(report_path, JSON.pretty_generate(report) + "\n")
checkouts.each_key do |path|
  stdout, stderr, status = Open3.capture3('git', "--git-dir=#{bare}", 'worktree', 'remove', '--force', path)
  raise "exact worktree removal failed: #{stderr}" unless status.success? && !File.exist?(path)
end
lint_paths.each { |path| FileUtils.remove_entry_secure(path) }
raise 'owned path still present' if owned_paths.any? { |path| File.exist?(path) }
report[:removed] = true
File.write(report_path, JSON.pretty_generate(report) + "\n")
File.write(File.join(docs, 'quality-ir-keys-cleanup.json'), JSON.pretty_generate(report) + "\n")
puts JSON.pretty_generate(report)
