require 'find'
require 'json'
require 'open3'
require 'pathname'
require 'time'

owned = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp/upstream-metadata-u9'
workspace = '/Users/sam/workspace/rtao'
raise 'unexpected cleanup path' unless File.realpath(owned) == owned && !File.symlink?(owned)
under = ->(path) { path == owned || path.start_with?(owned + '/') }
report = { 'audited_at' => Time.now.utc.iso8601, 'owned_root' => owned, 'errors' => [] }

processes, _, ok = Open3.capture3('/bin/ps', '-axo', 'pid=,ppid=,command=')
raise 'ps failed' unless ok.success?
rows = processes.lines.filter_map do |line|
  m = line.match(/^\s*(\d+)\s+(\d+)\s+(.*)$/)
  [m[1].to_i, m[2].to_i, m[3]] if m
end
ancestors = [Process.pid]
loop do
  row = rows.find { |pid, _, _| pid == ancestors.last }
  break unless row && row[1] > 0 && !ancestors.include?(row[1])
  ancestors << row[1]
end
report['process_references'] = rows.reject { |pid, _, cmd| ancestors.include?(pid) || !cmd.include?(owned) }.map { |pid, ppid, _| { 'pid' => pid, 'ppid' => ppid } }
report['excluded_audit_ancestor_pids'] = ancestors

out, err, status = Open3.capture3('/usr/sbin/lsof', '-nP', '+D', owned)
report['open_file_references'] = out.lines.drop(1).map { |line| line.split[0, 3] }
report['lsof_status'] = status.exitstatus
report['lsof_warnings'] = err.lines.map(&:strip).reject(&:empty?)
report['errors'] << 'lsof failed without a no-open-files status' unless [0, 1].include?(status.exitstatus)

report['launchd_references'] = {}
["gui/#{Process.uid}", 'system'].each do |domain|
  text, error, status = Open3.capture3('/bin/launchctl', 'print', domain)
  report['launchd_references'][domain] = text.lines.count { |line| line.include?(owned) }
  report['errors'] << "launchctl #{domain} failed: #{error.strip}" unless status.success?
end

roots = [workspace, '/Users/sam/.config', '/Users/sam/.cargo/git', '/Users/sam/Library/LaunchAgents', '/Library/LaunchAgents', '/Library/LaunchDaemons']
pruned = [workspace + '/target/debug', workspace + '/target/execute-plan.6Ro0PC/build', workspace + '/target/execute-plan.6Ro0PC/pin/upstream.git/.git/objects']
configs = ['/Users/sam/.cargo/config', '/Users/sam/.cargo/config.toml', '/Users/sam/.cargo/env', '/Users/sam/.zshrc', '/Users/sam/.zprofile', '/Users/sam/.bash_profile', '/Users/sam/.bashrc']
symlinks = []
refs = []
checked = []
roots.each do |root|
  next unless File.exist?(root)
  begin
    Find.find(root) do |path|
      if under.call(path) || pruned.include?(path) || (File.directory?(path) && ['objects', 'node_modules', '.venv'].include?(File.basename(path)))
        # Virtualenv dependency files must still be inspected before pruning.
        if File.basename(path) == '.venv'
          ['pyvenv.cfg', 'bin/activate', 'bin/python', 'bin/python3'].each do |suffix|
            target = File.join(path, suffix)
            configs << target if File.file?(target) && !File.symlink?(target)
            symlinks << target if File.symlink?(target)
          end
        end
        Find.prune if File.directory?(path)
        next
      end
      if File.symlink?(path)
        symlinks << path
        Find.prune if File.directory?(path)
      elsif File.file?(path)
        base = File.basename(path)
        configs << path if ['config', 'config.toml', 'pyvenv.cfg', 'activate', 'alternates', 'gitdir', 'commondir', '.env', '.envrc'].include?(base) || base.start_with?('.env.') || path.include?('/LaunchAgents/') || path.include?('/LaunchDaemons/') || path.start_with?('/Users/sam/.config/')
      end
    end
  rescue => e
    report['errors'] << "scan #{root}: #{e.class} #{e.message}"
  end
end
symlinks.uniq.each do |path|
  begin
    target = File.expand_path(File.readlink(path), File.dirname(path))
    resolved = File.exist?(path) ? File.realpath(path) : target
    refs << { 'kind' => 'symlink', 'path' => path } if under.call(target) || under.call(resolved)
  rescue => e
    report['errors'] << "symlink #{path}: #{e.class} #{e.message}"
  end
end
configs.uniq.each do |path|
  next unless File.file?(path) && !File.symlink?(path)
  begin
    next if File.size(path) > 2_000_000
    data = File.binread(path)
    checked << path
    refs << { 'kind' => 'configuration_or_environment', 'path' => path } if data.include?(owned)
  rescue => e
    report['errors'] << "config #{path}: #{e.class} #{e.message}"
  end
end
report['symlinks_inspected'] = symlinks.uniq.length
report['config_files_inspected'] = checked.length
report['external_references'] = refs
report['virtualenv_dependencies_created'] = false
report['git_alternates'] = File.read(owned + '/repo/.git/objects/info/alternates').strip
report['retained_live_dependencies'] = ['shared task pin cache', 'shared task build cache', 'shared task tools', 'other agents task directories']
report['disposable'] = report['process_references'].empty? && report['open_file_references'].empty? && report['launchd_references'].values.all?(&:zero?) && report['external_references'].empty? && report['errors'].empty?
File.write(owned + '/audit.json', JSON.pretty_generate(report) + "\n")
puts JSON.pretty_generate(report)
exit(report['disposable'] ? 0 : 1)
