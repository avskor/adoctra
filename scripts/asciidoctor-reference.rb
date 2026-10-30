# frozen_string_literal: true

# Always activate the pinned gem, even if a newer default version is installed.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'
# The rouge source highlighter loads the pinned Rouge (adoctra ports it).
gem 'rouge', File.read(File.expand_path('../compatibility/rouge-version', __dir__)).strip

report = ARGV.delete('--report')
structure = ARGV.delete('--structure')
semantics = ARGV.delete('--semantics')
positions = ARGV.delete('--diagnostic-positions')
abort 'report modes are mutually exclusive' if [report, structure, semantics, positions].count { |mode| mode } > 1
if report || structure || semantics || positions
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
# visual-v1 (scripts/visual.py): the complete standalone page of a fixture
# file, keeping Ruby's own embedded stylesheet, in safe mode from the file's
# directory; offline (no web fonts) and without the date-dependent footer.
# --embedded converts the body only (the harness adds Ruby's stylesheet);
# --visual-stylesheet prints that default stylesheet.
if ARGV.delete('--visual-stylesheet')
  print Asciidoctor::Stylesheets.instance.primary_stylesheet_data
  exit
end
if ARGV.delete('--visual')
  embedded = ARGV.delete('--embedded')
  abort 'usage: ruby scripts/asciidoctor-reference.rb --visual [--embedded] [--attribute NAME=VALUE]... FILE' unless ARGV.length == 1
  path = File.realpath(ARGV[0])
  attributes = { 'reproducible' => '', 'webfonts!' => '' }.merge(options[:attributes] || {})
  puts Asciidoctor.convert_file(path, safe: :safe, base_dir: File.dirname(path), to_file: false,
                                      header_footer: !embedded, attributes: attributes)
  exit
end
if semantics
  require_relative 'asciidoctor-semantics'
  converter = SemanticConverter.new 'html5'
  while (index = ARGV.index('--semantic-attribute'))
    converter.attribute_names << ARGV.slice!(index, 2).fetch(1)
  end
  options[:converter] = converter
  if ARGV.length == 2 && ARGV[0] == '--include-root'
    root = File.realpath(ARGV[1])
    input = File.read(File.join(root, 'main.adoc'))
    # These are the isolated text fixtures supplied by the harness.
    Dir.glob(File.join(root, '**', '*')).select { |path| File.file? path }.each do |path|
      abort 'reserved semantic handle character in input' if File.read(path).match?(/[\uE000\uE001]/)
    end
    options.merge! safe: :safe, base_dir: root, docfile: File.join(root, 'main.adoc')
  else
    abort 'unexpected semantic arguments' unless ARGV.empty?
    input = $stdin.read
  end
  abort 'reserved semantic handle character in input' if ([input] + (options[:attributes] || {}).values).any? { |s| s.match?(/[\uE000\uE001]/) }
  document = Asciidoctor.load(input, options)
  result = document.convert
  # This initial profile excludes diagnostics; never silently discard one.
  abort 'diagnostics are outside inline-semantics-v1' if logger.messages.any? { |entry| [:WARN, :ERROR, :FATAL, :UNKNOWN].include? entry[:severity] }
  puts JSON.generate(semantics: result)
  exit
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
if report || positions
  # The default report compares severity/text; positions adds logger cursors.
  # MemoryLogger records even calls below the normal WARN threshold.
  levels = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }
  diagnostics = logger.messages.filter_map do |entry|
    next unless (severity = levels[entry[:severity]])
    message = entry[:message]
    text = Hash === message ? message.fetch(:text) : message.to_s
    diagnostic = { severity: severity, message: text }
    if positions
      # A logger cursor is evidence; text mentioning a line is not a cursor.
      cursor = Hash === message ? message[:source_location] : nil
      diagnostic[:location] = cursor ? { file: cursor.file || cursor.path, line: cursor.lineno } : nil
    end
    diagnostic
  end
  if positions
    puts JSON.generate(diagnostics: diagnostics)
  else
    puts JSON.generate(html: output + (output.end_with?("\n") ? '' : "\n"), diagnostics: diagnostics)
  end
else
  puts output
end
