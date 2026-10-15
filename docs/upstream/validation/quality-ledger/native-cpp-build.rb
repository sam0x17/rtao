# Direct native build of the repository's CMake source list. No CMake/Hunter
# configuration, interpreter discovery, generated helper or container runs.
require 'json'
require 'open3'
require 'fileutils'
require 'digest'

audit = File.expand_path(__dir__)
source = File.join(audit, 'zondax')
build = File.join(audit, 'cpp-pr')
dependencies = File.join(audit, 'native-deps')
raise 'unexpected task root' unless audit == '/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/ledger-audit'
raise 'symlink build root' if File.symlink?(build)
FileUtils.mkdir_p(File.join(build, 'objects'))

archives = {
  'fmt-7.1.2.tar.gz' => [Digest::SHA1, '59bea0bd88e72ac2769c57d584b0cbed956038cf'],
  'googletest-1.14.0.tar.gz' => [Digest::SHA1, '2b28c2a3a30d86b1759543ef61fac3c4d69f8c4c'],
  'json-3.12.0.tar.xz' => [Digest::SHA256, '42f6e95cad6ec532fd372391373363b62a14af6d771056dbfc86160e6dfff7aa']
}
archives.each { |name, (algorithm, expected)| raise "archive mismatch: #{name}" unless algorithm.file(File.join(dependencies, name)).hexdigest == expected }

cmake = File.read(File.join(source, 'CMakeLists.txt'))
lib_block = cmake.match(/file\(GLOB_RECURSE LIB_SRC(.*?)\)\s*add_library\(app_lib/m)[1]
declared_c_files = lib_block.scan(/\$\{CMAKE_CURRENT_SOURCE_DIR\}([^\s)]+)/).flatten.map { |path| source + path }
# CMake's GLOB ignores absent entries (the pinned zxlib has no zbuffer.c).
c_files = declared_c_files.select { |path| File.file?(path) }
raise 'unexpected app source inventory' unless declared_c_files.length == 21 && c_files.length == 20
cpp_files = Dir.glob(File.join(source, 'tests', '**', '*.cpp')).sort
cpp_files += [
  File.join(dependencies, 'gtest/googletest/src/gtest-all.cc'),
  File.join(dependencies, 'gtest/googletest/src/gtest_main.cc'),
  File.join(dependencies, 'gtest/googlemock/src/gmock-all.cc'),
  File.join(dependencies, 'fmt/src/format.cc'),
  File.join(dependencies, 'fmt/src/os.cc')
]

includes = %w[deps/BLAKE2/ref deps/BLAKE3/c deps/ledger-zxlib/include deps/ledger-zxlib/app/ui app/src app/src/lib app/src/common].map { |p| File.join(source, p) }
includes += %w[fmt/include json/include gtest/googletest/include gtest/googletest gtest/googlemock/include gtest/googlemock].map { |p| File.join(dependencies, p) }
common = ['-O0', '-g', '-fno-omit-frame-pointer', '-pthread']
common += includes.flat_map { |p| ['-I', p] }
common += %w[-DBLAKE3_NO_SSE2 -DBLAKE3_NO_SSE41 -DBLAKE3_NO_AVX2 -DBLAKE3_NO_AVX512 -DBLAKE3_USE_NEON=0 -DLEDGER_MAJOR_VERSION=100 -DLEDGER_MINOR_VERSION=0]
common << "-DTESTVECTORS_DIR=\"#{source}/tests/\""
objects = []
commands = []
jobs = Queue.new
(c_files + cpp_files).each_with_index do |path, index|
  object = File.join(build, 'objects', "#{index}-#{File.basename(path)}.o")
  compiler = path.end_with?('.c') ? '/usr/bin/clang' : '/usr/bin/clang++'
  standard = path.end_with?('.c') ? '-std=gnu17' : '-std=gnu++17'
  arguments = [compiler, standard, *common, '-c', path, '-o', object]
  objects << object
  commands << arguments
  jobs << [index, arguments]
end
failures = Queue.new
workers = 4.times.map do
  Thread.new do
    loop do
      index, arguments = jobs.pop(true)
      stdout, stderr, status = Open3.capture3(*arguments)
      File.write(File.join(build, "compile-#{index}.log"), stdout + stderr)
      failures << [index, status.exitstatus] unless status.success?
    rescue ThreadError
      break
    end
  end
end
workers.each(&:join)
raise "compile failed: #{failures.pop.inspect}" unless failures.empty?
link = ['/usr/bin/clang++', '-pthread', *objects, '-o', File.join(build, 'unittests')]
stdout, stderr, status = Open3.capture3(*link)
File.write(File.join(build, 'link.log'), stdout + stderr)
raise 'native link failed' unless status.success?
File.write(File.join(build, 'native-command-provenance.json'), JSON.pretty_generate({sources: c_files + cpp_files, absent_glob_entries: declared_c_files - c_files, compile: commands, link: link, dependency_archives: archives.transform_values { |algorithm, value| {algorithm: algorithm.name, digest: value} }, cmake_or_hunter_executed: false, python_executed: false}))
puts "Direct native build complete: #{c_files.length} C files, #{cpp_files.length} C++ files."
