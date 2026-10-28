# frozen_string_literal: true

# A fresh process per probe keeps the optional OpenURI monkey patch isolated.
# Activate the gem, but let Asciidoctor load it lazily at the first cache-uri.
require 'json'
root = File.expand_path('..', __dir__)
gem 'asciidoctor', File.read("#{root}/compatibility/asciidoctor-version").strip
gem 'open-uri-cached', File.read("#{root}/compatibility/open-uri-cached-version").strip
require 'asciidoctor'
config = JSON.parse($stdin.read)
logger = Asciidoctor::MemoryLogger.new
Asciidoctor::LoggerManager.logger = logger
result = {}
begin
  document = Asciidoctor.load_file(config.fetch('file'), safe: config.fetch('safe').to_sym,
                                 attributes: config.fetch('attributes'))
  result[:html] = document.convert
rescue StandardError => e
  # Expose exceptions separately; the audit must never count these as PASS.
  result[:error] = { class: e.class.name, message: e.message }
ensure
  OpenURI::Cache.invalidate_all! if defined? OpenURI::Cache
end
result[:diagnostics] = logger.messages.filter_map do |entry|
  severity = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }[entry[:severity]]
  next unless severity
  message = entry[:message]
  cursor = Hash === message ? message[:source_location] : nil
  { severity: severity, message: Hash === message ? message.fetch(:text) : message.to_s,
    location: cursor ? { file: cursor.file || cursor.path, line: cursor.lineno } : nil }
end
puts JSON.generate(result)
