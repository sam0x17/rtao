require 'digest'
require 'json'
require 'open3'

pr = ARGV.fetch(0)
branch = {'3246'=>'sdk-portable-protocol', '3247'=>'sdk-receipt-nested-failures'}.fetch(pr)
dir = File.expand_path(File.dirname(__FILE__))
head = File.read("#{dir}/pr-#{pr}-head.txt").strip
def command(*args)
  out, err, status = Open3.capture3(*args)
  abort "verification failed #{args.first}: #{err}" unless status.success?
  out
end
login = command('gh','api','user','--jq','.login').strip
abort 'GitHub identity changed' unless login == 'sam0x17'
remote = command('git','ls-remote','git@github.com:sam0x17/subtensor.git',"refs/heads/#{branch}")
abort 'remote SHA mismatch' unless remote.strip == "#{head}\trefs/heads/#{branch}"
File.write("#{dir}/pr-#{pr}-remote.txt",remote)
json = command('gh','pr','view',pr,'--repo','RaoFoundation/subtensor','--json','number,title,body,url,headRefOid,headRefName,headRepositoryOwner,baseRefName,state,labels,reviewRequests')
current = JSON.parse(json)
before = JSON.parse(File.read("#{dir}/pr-#{pr}-before.json"))
abort 'PR SHA mismatch' unless current['headRefOid'] == head
%w[number title url headRefName headRepositoryOwner baseRefName state labels reviewRequests].each do |key|
  abort "unexpected PR change #{key}" unless current[key] == before[key]
end
abort 'labels or review requests present' unless current['labels'].empty? && current['reviewRequests'].empty?
File.write("#{dir}/pr-#{pr}-after.json", json)
body = File.read("#{dir}/pr-#{pr}-body.md")
abort 'PR body differs' unless current['body'] == body
%w[pre-push push].each do |kind|
  path = "#{dir}/pr-#{pr}-#{kind}.log"
  abort "missing completed #{kind} log" unless File.file?(path)
end
abort 'completed clean-head gate missing' unless File.read("#{dir}/pr-#{pr}-pre-push.log").include?("Clean committed HEAD gate passed: PR #{pr} #{head}")
repository = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/subtensor.git'
commit = command('git','-C',repository,'log','-1',head,'--format=%H%n%an <%ae>%n%cn <%ce>%n%aI%n%cI')
record = {
  pr: pr.to_i, url: current['url'], head: head, fork: 'sam0x17/subtensor', branch: branch,
  github_actor: login, ssh_actor: 'sam0x17', commit_identity: commit.lines.map(&:strip),
  original_head: before['headRefOid'], publication: 'ordinary protected-hook push; no force, rebase, hook bypass or persistent Git configuration change',
  body_byte_verified: true, body_sha256: Digest::SHA256.hexdigest(body),
  labels: current['labels'], review_requests: current['reviewRequests'],
  final_log_hashes: %w[pre-push push].to_h { |kind| [kind, Digest::SHA256.file("#{dir}/pr-#{pr}-#{kind}.log").hexdigest] },
  local_python: 'No interpreter used for this correction; every Cargo command sets PYO3_NO_PYTHON=1.'
}
File.write("#{dir}/pr-#{pr}-publication.json", JSON.pretty_generate(record) + "\n")
puts JSON.generate(record)
