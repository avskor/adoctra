# frozen_string_literal: true

# Record every top-level document the pinned Asciidoctor test suite builds.
#
#   ASCIIDOCTOR_TEST_DIR=<upstream>/test RECORD_OUT=out.jsonl \
#     ruby -r ./scripts/upstream_record.rb <upstream>/test/blocks_test.rb
#
# Asciidoctor::Document#initialize is wrapped: each document without a parent
# (helpers such as convert_string and direct Asciidoctor.load/convert calls)
# appends one JSON line with its source, the options the test passed (before
# the helper's defaults are known to the document they are included), the
# calling test file/line and the enclosing `test '…' do` declaration. Tests run
# normally; their pass/fail status is irrelevant to the recording.
require 'json'

test_dir = ENV.fetch('ASCIIDOCTOR_TEST_DIR')
OUT = File.open(ENV.fetch('RECORD_OUT'), 'a')
OUT.sync = true
require File.join(test_dir, 'test_helper')

module UpstreamRecorder
  DECLARATIONS = Hash.new do |cache, path|
    cache[path] = File.readlines(path).each_with_index.filter_map do |line, index|
      (match = line.match(/^\s*test (['"])(.*)\1 do\s*$/)) && [index + 1, match[2]]
    end
  end

  def self.plain(value)
    case value
    when Hash then value.to_h { |k, v| [k.to_s, plain(v)] }
    when Array then value.map { |v| plain(v) }
    when Symbol then value.to_s
    when String, Integer, Float, true, false, nil then value
    else "#<#{value.class}>"
    end
  end

  def initialize data = nil, options = {}
    unless options[:parent]
      frame = caller_locations.find { |location| location.path.end_with? '_test.rb' }
      if frame
        declaration = DECLARATIONS[frame.path].reverse.find { |line, _| line <= frame.lineno }
        source = case data
                 when Array then data.join
                 when String then data
                 when nil then ''
                 else (data.respond_to? :read) ? '#<IO>' : "#<#{data.class}>"
                 end
        OUT.puts JSON.generate(
          file: File.basename(frame.path), call_line: frame.lineno,
          test_line: declaration&.first, test: declaration&.last,
          source: source, options: UpstreamRecorder.plain(options)
        )
      end
    end
    super
  end
end

Asciidoctor::Document.prepend UpstreamRecorder
