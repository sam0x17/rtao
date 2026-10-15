require 'digest'
require 'json'

pr = ARGV.fetch(0)
abort 'unexpected PR' unless %w[3246 3247].include?(pr)
dir = File.expand_path(File.dirname(__FILE__))
root = '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC'
records = %w[1.89.0 stable].flat_map do |toolchain|
  %w[portable lib].flat_map do |kind|
    log = File.read("#{dir}/pr-#{pr}-#{toolchain}-#{kind}.log")
    paths = log.scan(/Running [^\n]+ \(([^\n)]+)\)/).flatten
    expected = kind == 'lib' ? 1 : (pr == '3246' ? 3 : 4)
    abort "artifact count mismatch #{pr} #{toolchain} #{kind}" unless paths.length == expected
    paths.map do |path|
      abort "unexpected artifact path #{path}" unless path.start_with?("#{root}/upstream-quality/unicode-publication/pr-#{pr}/functional/") && File.file?(path)
      depfile = path + '.d'
      abort "missing artifact dependency #{depfile}" unless File.file?(depfile)
      dep = File.read(depfile)
      source_refs = dep.scan(%r{#{Regexp.escape(root)}/upstream-quality/pr-\d+/[^\s:]+}).uniq
      abort 'foreign checkout artifact' unless source_refs.all? { |ref| ref.start_with?("#{root}/upstream-quality/pr-#{pr}/") }
      # Cargo's rustc dependency files use workspace-relative input paths here.
      # Resolve them against the exclusive checkout recorded by the build log.
      checkout = "#{root}/upstream-quality/pr-#{pr}"
      relative_sources = dep.scan(%r{(?<!\S)(sdk/bittensor-core/[^\s:]+)}).flatten.uniq
      abort 'artifact lacks workspace source references' if relative_sources.empty?
      source_hashes = relative_sources.to_h do |relative|
        source = File.join(checkout, relative)
        abort "missing checkout source #{source}" unless File.file?(source)
        [relative, Digest::SHA256.file(source).hexdigest]
      end
      if kind == 'lib'
        abort 'Unicode shipping table missing from library artifact dependencies' unless dep.include?('/src/protocol/python_printable.rs')
        abort 'receipt artifact mismatch' unless dep.include?('/src/protocol/receipt.rs') == (pr == '3247')
      end
      {
        toolchain: toolchain, kind: kind, path: path, sha256: Digest::SHA256.file(path).hexdigest,
        bytes: File.size(path), dependency_file: depfile, dependency_sha256: Digest::SHA256.file(depfile).hexdigest,
        own_checkout: checkout, absolute_source_references: source_refs,
        workspace_relative_source_hashes: source_hashes
      }
    end
  end
end
abort 'artifact identities collide between compilers' unless records.map { |record| record[:path] }.uniq.length == records.length
File.write("#{dir}/pr-#{pr}-artifacts.json", JSON.pretty_generate(records) + "\n")
puts JSON.generate(records.map { |record| record.slice(:toolchain, :kind, :path, :sha256) })
