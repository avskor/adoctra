# frozen_string_literal: true

# Reference side of scripts/upstream_audit.py: convert every inventory entry
# with the pinned Asciidoctor in one process, using the options its upstream
# test passed (as recorded), and print one JSON line per entry:
#   {"id", "html", "diagnostics": [{"severity", "message"}]} or {"id", "error"}.
# Usage: ruby scripts/upstream-audit-reference.rb ENTRIES.json
require 'json'

# The reference environment has no optional server-side highlighter gems
# (like the golden references and CI), whatever this machine has installed:
# Asciidoctor then logs that the gem is unavailable, as it would there (a
# missing gem's LoadError names its path, so the message has no reason).
module UnavailableHighlighters
  NAMES = %w(rouge coderay pygments pygments.rb).freeze

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
SAFE = { 'unsafe' => :unsafe, 'safe' => :safe, 'server' => :server, 'secure' => :secure }.freeze

def options_of recorded
  recorded.each_with_object({}) do |(key, value), options|
    case key
    when 'safe' then options[:safe] = (SAFE[value] || value)
    when 'standalone', 'header_footer', 'parse', 'attributes', 'doctype', 'base_dir'
      options[key.to_sym] = value
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
    output = (Asciidoctor::Document.new entry['source'], (options_of entry['options'])).convert
    # What Ruby's CLI writes (the other side is the CLI): the output chomped
    # plus a LF, nothing for nil (the inline doctype without a candidate).
    html = output.nil? ? '' : %(#{output.chomp}\n)
    diagnostics = logger.messages.filter_map do |message|
      next unless (severity = LEVELS[message[:severity]])
      text = message[:message]
      { severity: severity, message: (Hash === text ? text.fetch(:text) : text.to_s) }
    end
    puts JSON.generate(id: entry['id'], html: html, diagnostics: diagnostics)
  rescue StandardError, SystemStackError => e
    puts JSON.generate(id: entry['id'], error: "#{e.class}: #{e.message.lines.first&.chomp}")
  ensure
    OpenURI::Cache.invalidate_all! if defined? OpenURI::Cache
  end
end
