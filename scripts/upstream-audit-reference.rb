# frozen_string_literal: true

# Reference side of scripts/upstream_audit.py: convert every inventory entry
# with the pinned Asciidoctor in one process, using the options its upstream
# test passed (as recorded), and print one JSON line per entry:
#   {"id", "html", "diagnostics": [{"severity", "message", "location"}],
#    "model": {"structure", "attributes": {"header", "final"}}} or {"id", "error"}.
# Usage: ruby scripts/upstream-audit-reference.rb ENTRIES.json
require 'json'

# The reference environment has the pinned Rouge (compatibility/rouge-version,
# which adoctra ports) but no other optional server-side highlighter gems
# (like the golden references and CI), whatever this machine has installed:
# Asciidoctor then logs that the gem is unavailable, as it would there (a
# missing gem's LoadError names its path, so the message has no reason).
gem 'rouge', File.read(File.expand_path('../compatibility/rouge-version', __dir__)).strip

module UnavailableHighlighters
  NAMES = %w(coderay pygments pygments.rb).freeze

  def require name
    if NAMES.include?(feature = name.to_s.split('/').first)
      error = LoadError.new "cannot load such file -- #{name}"
      error.define_singleton_method(:path) { feature }
      raise error
    end
    super
  end
end
Kernel.prepend UnavailableHighlighters
Object.prepend UnavailableHighlighters

version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'
require 'tmpdir'
# Asciidoctor loads open-uri-cached lazily at the first `cache-uri`; activate
# the pinned version (CI installs it) when present.
begin
  gem 'open-uri-cached', File.read(File.expand_path('../compatibility/open-uri-cached-version', __dir__)).strip
rescue Gem::LoadError
  nil
end

LEVELS = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }.freeze

def logged logger
  logger.messages.filter_map do |message|
    next unless (severity = LEVELS[message[:severity]])
    text = message[:message]
    # The logger cursor, when the message has one (text mentioning a line
    # is no cursor): the harness compares it with the CLI's file:line.
    cursor = Hash === text ? text[:source_location] : nil
    { severity: severity, message: (Hash === text ? text.fetch(:text) : text.to_s),
      location: cursor ? { file: cursor.file || cursor.path, line: cursor.lineno } : nil }
  end
end

# `--invoke ARGS...`: one run of Ruby's CLI (Asciidoctor::Cli::Invoker) in
# the current directory, standard input as the document for `-`, with the
# same highlighter environment as the batch mode. Prints one JSON object:
# exit code, standard output, the error text and the logged diagnostics
# (the harness collects the files the run wrote).
if ARGV[0] == '--invoke'
  require 'asciidoctor/cli'
  require 'stringio'
  logger = Asciidoctor::MemoryLogger.new
  Asciidoctor::LoggerManager.logger = logger
  invoker = Asciidoctor::Cli::Invoker.new ARGV.drop 1
  invoker.redirect_streams (out = StringIO.new), (err = StringIO.new)
  invoker.invoke! { $stdin.read }
  puts JSON.generate(code: invoker.code, stdout: out.string, error: err.string, diagnostics: (logged logger))
  exit
end
SAFE = { 'unsafe' => :unsafe, 'safe' => :safe, 'server' => :server, 'secure' => :secure }.freeze

def options_of recorded
  recorded.each_with_object({}) do |(key, value), options|
    case key
    when 'safe' then options[:safe] = (SAFE[value] || value)
    when 'standalone', 'header_footer', 'parse', 'attributes', 'doctype', 'base_dir', 'backend',
         'sourcemap', 'catalog_assets', 'eruby'
      options[key.to_sym] = value
    # Recorded as the object's class: a fresh collector, as the test passed.
    when 'timings' then options[:timings] = Asciidoctor::Timings.new
    end
  end
end

# Each entry must behave like its own process (the CLI side runs one per
# document): highlighter adapters (loaded lazily) cache their library status
# per process, so a missing gem would warn only for the first document.
def reset_highlighter_status
  ObjectSpace.each_object(Module) do |mod|
    next unless mod.name&.start_with?('Asciidoctor::SyntaxHighlighter')
    mod.class_variable_set :@@library_status, nil if mod.class_variable_defined? :@@library_status
  end
end

require_relative 'asciidoctor-structure'

def attribute_strings attributes
  (attributes || {}).to_h {|name, value| [name.to_s, value.nil? ? nil : value.to_s] }
end

entries = JSON.parse(File.read(ARGV.fetch(0)))
entries.each do |entry|
  reset_highlighter_status
  # Like separate processes: once loaded, open-uri-cached stays patched in,
  # so every entry starts from its own empty cache (the upstream tests use
  # a temporary cache_path too).
  OpenURI::Cache.cache_path = Dir.mktmpdir if defined? OpenURI::Cache
  logger = Asciidoctor::MemoryLogger.new
  Asciidoctor::LoggerManager.logger = logger
  begin
    document = Asciidoctor::Document.new entry['source'], (options_of entry['options'])
    output = document.convert
    # What Ruby's CLI writes (the other side is the CLI): the output chomped
    # plus a LF, nothing for nil (the inline doctype without a candidate).
    html = output.nil? ? '' : %(#{output.chomp}\n)
    diagnostics = logged logger
    # The document model the upstream assertions read through the API: the
    # block structure (block-structure-v1) and the attributes when the header
    # was saved (Document#save_attributes) and after conversion.
    model = { structure: (structure_document document),
              attributes: { header: (attribute_strings document.instance_variable_get :@header_attributes),
                            final: (attribute_strings document.attributes) } }
    # Deeply nested lists exceed JSON's default nesting limit.
    puts JSON.generate({ id: entry['id'], html: html, diagnostics: diagnostics, model: model }, max_nesting: false)
  rescue StandardError, SystemStackError => e
    puts JSON.generate(id: entry['id'], error: "#{e.class}: #{e.message.lines.first&.chomp}")
  ensure
    OpenURI::Cache.invalidate_all! if defined? OpenURI::Cache
  end
end
