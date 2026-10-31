# frozen_string_literal: true

# A persistent reference worker for scripts/fuzz.py: one JSON request per
# stdin line ({"source": "..."}), one JSON reply per stdout line with the same
# report as `asciidoctor-reference.rb --report --showtitle` (secure, embedded)
# or {"error": "..."} when the pinned Asciidoctor raises. Starting Ruby once
# keeps differential fuzzing and minimization affordable.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'
gem 'rouge', File.read(File.expand_path('../compatibility/rouge-version', __dir__)).strip
require 'json'

LEVELS = { WARN: 'warning', ERROR: 'error', FATAL: 'fatal', UNKNOWN: 'unknown' }.freeze

$stdout.sync = true
$stdin.each_line do |line|
  request = JSON.parse(line)
  logger = Asciidoctor::MemoryLogger.new
  Asciidoctor::LoggerManager.logger = logger
  reply =
    begin
      output = Asciidoctor.convert(request.fetch('source'), safe: :secure, header_footer: false,
                                                             attributes: { 'showtitle' => '' })
      diagnostics = logger.messages.filter_map do |entry|
        next unless (severity = LEVELS[entry[:severity]])
        message = entry[:message]
        { severity: severity, message: Hash === message ? message.fetch(:text) : message.to_s }
      end
      { html: output + (output.end_with?("\n") ? '' : "\n"), diagnostics: diagnostics }
    rescue StandardError, SystemStackError => e
      { error: "#{e.class}: #{e.message.lines.first&.chomp}" }
    end
  puts JSON.generate(reply)
end
