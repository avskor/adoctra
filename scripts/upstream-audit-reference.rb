# frozen_string_literal: true

# Reference side of scripts/upstream_audit.py: convert every inventory entry
# with the pinned Asciidoctor in one process, using the options its upstream
# test passed (as recorded), and print one JSON line per entry:
#   {"id", "html", "diagnostics": [{"severity", "message"}]} or {"id", "error"}.
# Usage: ruby scripts/upstream-audit-reference.rb ENTRIES.json
require 'json'

# The reference environment has no optional server-side highlighter gems
# (like the golden references and CI), whatever this machine has installed:
# Asciidoctor then logs that the gem is unavailable, as it would there.
module UnavailableHighlighters
  NAMES = %w(rouge coderay pygments pygments.rb).freeze

  def require name
    raise LoadError, "cannot load such file -- #{name}" if NAMES.include? name.to_s.split('/').first
    super
  end
end
Kernel.prepend UnavailableHighlighters
Object.prepend UnavailableHighlighters

version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'

LEVELS = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }.freeze
SAFE = { 'unsafe' => :unsafe, 'safe' => :safe, 'server' => :server, 'secure' => :secure }.freeze

def options_of recorded
  recorded.each_with_object({}) do |(key, value), options|
    case key
    when 'safe' then options[:safe] = (SAFE[value] || value)
    when 'standalone', 'header_footer', 'parse', 'attributes', 'doctype'
      options[key.to_sym] = value
    end
  end
end

entries = JSON.parse(File.read(ARGV.fetch(0)))
entries.each do |entry|
  logger = Asciidoctor::MemoryLogger.new
  Asciidoctor::LoggerManager.logger = logger
  begin
    html = (Asciidoctor::Document.new entry['source'], (options_of entry['options'])).convert.to_s
    diagnostics = logger.messages.filter_map do |message|
      next unless (severity = LEVELS[message[:severity]])
      text = message[:message]
      { severity: severity, message: (Hash === text ? text.fetch(:text) : text.to_s) }
    end
    puts JSON.generate(id: entry['id'], html: html, diagnostics: diagnostics)
  rescue StandardError, SystemStackError => e
    puts JSON.generate(id: entry['id'], error: "#{e.class}: #{e.message.lines.first&.chomp}")
  end
end
