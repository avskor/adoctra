# frozen_string_literal: true

# Always activate the pinned gem, even if a newer default version is installed.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'

options = { safe: :secure, header_footer: false }
options[:attributes] = { 'showtitle' => '' } if ARGV == ['--showtitle']
abort 'usage: ruby scripts/asciidoctor-reference.rb [--showtitle] < input.adoc' unless ARGV.empty? || ARGV == ['--showtitle']
puts Asciidoctor.convert($stdin.read, options)
