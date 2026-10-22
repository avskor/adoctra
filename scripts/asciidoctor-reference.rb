# frozen_string_literal: true

# Always activate the pinned gem, even if a newer default version is installed.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'

report = ARGV.delete('--report')
structure = ARGV.delete('--structure')
abort '--report and --structure are mutually exclusive' if report && structure
if report || structure
  require 'json'
  logger = Asciidoctor::MemoryLogger.new
  Asciidoctor::LoggerManager.logger = logger
end

options = { safe: :secure, header_footer: false }
options[:attributes] = { 'showtitle' => '' } if ARGV.delete('--showtitle')
# Safe mode keeps attributes secure mode locks (e.g. source-highlighter).
options[:safe] = :safe if ARGV.delete('--safe')
mathjax_config = ARGV.delete('--mathjax-config')
options[:header_footer] = true if mathjax_config
# A standalone document without the date-dependent footer line and the
# stylesheet, for comparing the document shell (head, header and footer).
standalone = ARGV.delete('--standalone')
if standalone
  options[:header_footer] = true
  options[:attributes] = (options[:attributes] || {}).merge('reproducible' => '')
end
# Host attributes (`--attribute name=value`), like the CLI's `-a`.
while (index = ARGV.index('--attribute'))
  name, value = ARGV.slice!(index, 2)[1].split('=', 2)
  options[:attributes] = (options[:attributes] || {}).merge(name => value || '')
end
if structure
  require_relative 'asciidoctor-structure'
  if ARGV.length == 2 && ARGV[0] == '--include-root'
    root = File.realpath(ARGV[1])
    document = Asciidoctor.load_file(File.join(root, 'main.adoc'), options.merge(safe: :safe, base_dir: root))
  else
    abort 'unexpected structure arguments' unless ARGV.empty?
    document = Asciidoctor.load($stdin.read, options)
  end
  puts JSON.generate(structure: structure_document(document))
  exit
end
if ARGV.length == 2 && ARGV[0] == '--include-root'
  root = File.realpath(ARGV[1])
  options.merge! safe: :safe, base_dir: root, to_file: false
  output = Asciidoctor.convert_file(File.join(root, 'main.adoc'), options)
else
  abort 'usage: ruby scripts/asciidoctor-reference.rb [--report] [--showtitle] [--safe] [--mathjax-config] [--standalone] [--attribute NAME=VALUE]... [--include-root DIR] < input.adoc' unless ARGV.empty?
  output = Asciidoctor.convert($stdin.read, options)
  if standalone
    # Only Ruby's own stylesheet and its web fonts; other links (e.g. a
    # syntax highlighter theme) are compared.
    output = output.gsub(/<link rel="stylesheet" href="[^"]*(?:asciidoctor\.css|fonts\.googleapis\.com[^"]*)">\n/, '')
                   .sub(/<style>\n.*?<\/style>\n/m, '')
  end
  if mathjax_config
    output = output[/<script type="text\/x-mathjax-config">.*?<\/script>\n<script src="[^"]+\/MathJax\.js[^\"]*"><\/script>/m] or abort 'MathJax configuration not found'
  end
end
if report
  # This profile deliberately compares severity and text, not source positions.
  # MemoryLogger records even calls below the normal WARN threshold.
  levels = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }
  diagnostics = logger.messages.filter_map do |entry|
    next unless (severity = levels[entry[:severity]])
    message = entry[:message]
    text = Hash === message ? message.fetch(:text) : message.to_s
    { severity: severity, message: text }
  end
  puts JSON.generate(html: output + (output.end_with?("\n") ? '' : "\n"), diagnostics: diagnostics)
else
  puts output
end
