# frozen_string_literal: true

# Extend compatibility/encoding-order/encodings.json, the byte fragments of the
# include decoding/selection order audit (scripts/audit_include_order.py), with
# every encoding of the pinned Ruby that converts to UTF-8. Profiles already in
# the file are kept byte for byte. Run from the repository root:
#   ruby scripts/generate_encoding_order.rb
# Each fragment is encoded by Ruby; `bad` is the first candidate byte sequence
# whose conversion to UTF-8 fails after a valid line (nil when every candidate
# converts, as in single-byte encodings that map all 256 bytes).
require 'json'

path = File.expand_path('../compatibility/encoding-order/encodings.json', __dir__)
profiles = JSON.parse File.read(path)
FRAGMENTS = ["*ok*\r\n", 'A', "\nlast\n", "// tag::body[]\n*ok*\n// end::body[]\n"].freeze
# Decoded CR, NEL and LS stay inside a line (Ruby splits lines at LF only).
# Added to every profile, existing ones too; null where the encoding cannot
# represent the text.
CONTROLS = ["a\rb\nc\n", "a\u0085b\nc\n", "a\u2028b\nc\n"].freeze
CANDIDATES = ["\xff", "\x80", "\x81", "\x8f", "\x98", "\xa0", "\xfe", "\x1b$X"].map { |s| s.b }.freeze

def converts? name, bytes
  converter = Encoding::Converter.new name, 'UTF-8'
  converter.convert(bytes.dup.force_encoding(name)) + converter.finish
  true
rescue Encoding::InvalidByteSequenceError, Encoding::UndefinedConversionError
  false
end

Encoding.list.map(&:name).sort.each do |name|
  if profiles.key? name
    CONTROLS.each do |text|
      profiles[name]['fragments'][text] ||= begin
        text.encode(name).b.unpack1('H*')
      rescue Encoding::UndefinedConversionError
        nil
      end
    end
    next
  end
  begin
    Encoding::Converter.new name, 'UTF-8'
    fragments = FRAGMENTS.to_h { |text| [text, text.encode(name).b.unpack1('H*')] }
  rescue Encoding::ConverterNotFoundError, Encoding::UndefinedConversionError
    next
  end
  first = [fragments["*ok*\r\n"]].pack('H*')
  next unless converts? name, first
  bad = CANDIDATES.find { |candidate| !(converts? name, first + candidate) }
  profiles[name] = { 'bom' => '', 'fragments' => fragments, 'bad' => bad&.unpack1('H*') }
end

File.write path, %(#{JSON.pretty_generate profiles}\n)
warn "#{profiles.size} profiles"
