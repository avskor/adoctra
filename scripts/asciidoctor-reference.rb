# frozen_string_literal: true

# Always activate the pinned gem, even if a newer default version is installed.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'

options = { safe: :secure, header_footer: false }
options[:attributes] = { 'showtitle' => '' } if ARGV.delete('--showtitle')
if ARGV.length == 2 && ARGV[0] == '--include-root'
  root = File.realpath(ARGV[1])
  options.merge! safe: :safe, base_dir: root, to_file: false
  puts Asciidoctor.convert_file(File.join(root, 'main.adoc'), options)
else
  abort 'usage: ruby scripts/asciidoctor-reference.rb [--showtitle] [--include-root DIR] < input.adoc' unless ARGV.empty?
  puts Asciidoctor.convert($stdin.read, options)
end
