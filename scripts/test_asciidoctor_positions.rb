# frozen_string_literal: true
# Integration checks for the pinned logger exporter, independent of adoctra.
require 'json'
require 'open3'
require 'rbconfig'
require 'tmpdir'
require 'fileutils'

LAUNCHER = File.expand_path('asciidoctor-reference.rb', __dir__)

def project(source, *args)
  stdout, stderr, status = Open3.capture3(RbConfig.ruby, LAUNCHER, '--diagnostic-positions', *args, stdin_data: source)
  raise "reference failed: #{stderr}" unless status.success? && stderr.empty?
  report = JSON.parse(stdout)
  raise 'unexpected report fields' unless report.keys == ['diagnostics']
  report.fetch('diagnostics')
end

def assert(condition, message)
  raise message unless condition
end

cases = {
  'stdin cursor has a physical line, no invented column or byte span' => lambda do
    diagnostic = project("= Пример\r\n\r\n=== Пропуск\r\n\r\nТекст.\r\n").fetch(0)
    assert(diagnostic == { 'severity' => 'warning',
                           'message' => 'section title out of sequence: expected level 1, got level 2',
                           'location' => { 'file' => '<stdin>', 'line' => 3 } }, 'wrong logger cursor')
  end,
  'unlocated conversion warning stays null' => lambda do
    diagnostics = project(":attribute-missing: warn\n\nЖ {missing}")
    assert(diagnostics.size == 1 && diagnostics[0]['location'].nil?, 'invented cursor for string message')
    assert(diagnostics[0]['message'] == 'skipping reference to missing attribute: missing', 'missing conversion warning')
  end,
  'normal WARN threshold excludes informational xref output' => lambda do
    assert(project('See <<missing>>.').empty?, 'info leaked into WARN profile')
  end,
  'repeated warnings retain order and line numbers' => lambda do
    diagnostics = project("= T\n\n=== First\n\ntext\n\n===== Second\n\ntext\n")
    assert(diagnostics.map { |d| d['location']['line'] } == [3, 7], 'lost or reordered cursors')
  end,
  'nested tag selection exports absolute physical filename' => lambda do
    Dir.mktmpdir do |root|
      FileUtils.mkdir_p(File.join(root, 'sub'))
      File.write(File.join(root, 'main.adoc'), "= T\n\ninclude::sub/outer.adoc[]\n")
      File.write(File.join(root, 'sub/outer.adoc'), "include::inner.adoc[tag=body]\n")
      File.write(File.join(root, 'sub/inner.adoc'), "ignored\n// tag::body[]\n=== Skip\n\nText\n// end::body[]\n")
      diagnostic = project('', '--include-root', root).fetch(0)
      assert(diagnostic['location'] == { 'file' => File.realpath(File.join(root, 'sub/inner.adoc')), 'line' => 3 }, 'lost include cursor')
    end
  end,
  'report modes are mutually exclusive' => lambda do
    _, stderr, status = Open3.capture3(RbConfig.ruby, LAUNCHER, '--report', '--diagnostic-positions', stdin_data: '')
    assert(!status.success? && stderr.include?('mutually exclusive'), 'accepted conflicting modes')
  end,
}

cases.each do |name, check|
  check.call
  puts "PASS #{name}"
end
puts "#{cases.size} diagnostic reference checks passed"
