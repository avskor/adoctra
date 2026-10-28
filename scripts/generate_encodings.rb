# frozen_string_literal: true

# Reproduce the byte-to-Unicode tables and encoding names used by the pinned
# Ruby reference. No Ruby code runs in adoctra. Run from the repository root:
# ruby scripts/generate_encodings.rb > crates/adoctra-core/src/source_encoding_tables.rs
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'
require 'unicode_normalize/normalize'

JAPANESE = %w[Shift_JIS Windows-31J EUC-JP eucJP-ms CP51932].freeze
BIG5 = %w[Big5 Big5-HKSCS Big5-UAO CP950 CP951].freeze
GB = %w[GB2312 GB12345 GBK GB18030].freeze
KOREAN = %w[EUC-KR CP949].freeze
JIS2004 = %w[EUC-JIS-2004].freeze
SJIS_MOBILE = %w[SJIS-KDDI SJIS-DoCoMo SJIS-SoftBank].freeze
UTF8_MOBILE = %w[UTF8-DoCoMo UTF8-KDDI UTF8-SoftBank].freeze
MULTIBYTE = (JAPANESE + BIG5 + GB + KOREAN + JIS2004).freeze
UTF8_MAC_PROBES = (UnicodeNormalize::DECOMPOSITION_TABLE.values + UnicodeNormalize::COMPOSITION_TABLE.keys).uniq.sort.freeze

# Independent exhaustive reference for the Rust decoder regression test,
# including invalid input and sequences containing multiple ASCII characters.
# This enumerates all byte pairs, rather than the table generator's lead/trail
# ranges. EUC also tests every pair following the three-byte prefix 0x8f.
if ARGV == ['--fingerprints']
  (MULTIBYTE + ['GB18030-four', 'ISO-2022-JP', 'CP50220', 'CP50221', 'stateless-ISO-2022-JP', 'ISO-2022-JP-KDDI', 'stateless-ISO-2022-JP-KDDI'] + SJIS_MOBILE + UTF8_MOBILE + ['CESU-8', 'UTF8-MAC']).each do |label|
    name = label == 'GB18030-four' ? 'GB18030' : label
    hash = 0xcbf29ce484222325
    feed = lambda { |byte| hash = ((hash ^ byte) * 0x100000001b3) & 0xffffffffffffffff }
    check = lambda do |bytes|
      begin
        input = bytes.pack('C*').force_encoding(name)
        result = if name == 'stateless-ISO-2022-JP' || name.end_with?('-KDDI') || JIS2004.include?(name) || SJIS_MOBILE.include?(name) || UTF8_MOBILE.include?(name) || %w[CESU-8 UTF8-MAC].include?(name)
                   # Like Ruby IO, always run the transcoder. String#encode
                   # bypasses it on ASCII-only strings, hiding ESC/SO/SI errors.
                   converter = Encoding::Converter.new name, 'UTF-8'
                   converter.convert(input) + converter.finish
                 else
                   input.encode('UTF-8')
                 end
        feed.call 1
        result.bytes.each { |byte| feed.call byte }
        feed.call 255
      rescue EncodingError
        feed.call 0
      end
    end
    if %w[ISO-2022-JP CP50220 CP50221 ISO-2022-JP-KDDI].include? label
      # Every single byte/pair in every designation, including errors. Every
      # escape candidate is followed by text valid in both ASCII and JIS, so
      # the digest also distinguishes the resulting state after a transition.
      prefixes = [[], [27, 36, 66], [27, 36, 64], [27, 40, 66], [27, 40, 74]]
      if label.start_with? 'CP5022'
        prefixes.concat [[27, 40, 73], [14], [15], [27, 36, 66, 14], [27, 36, 66, 15], [27, 40, 73, 15], [27, 40, 74, 14]]
      end
      prefixes.each do |prefix|
        check.call prefix
        256.times { |a| check.call prefix + [a] }
        256.times { |a| 256.times { |b| check.call prefix + [a, b] } }
        256.times { |a| 256.times { |b| check.call prefix + [27, a, b, 36, 34] } }
      end
    elsif label == 'GB18030-four'
      # Independent of the range generator's linear pointer enumeration:
      # every syntactic four-byte sequence and every truncated three-byte prefix.
      (0x81..0xfe).each do |a|
        (0x30..0x39).each do |b|
          (0x81..0xfe).each do |c|
            check.call [a, b, c]
            (0x30..0x39).each { |d| check.call [a, b, c, d] }
          end
        end
      end
      [[0x81, 0x30, 0x81, 0x30], [0x90, 0x30, 0x81, 0x30], [0xe3, 0x32, 0x9a, 0x35]].each do |base|
        4.times { |index| 256.times { |byte| input = base.dup; input[index] = byte; check.call input } }
      end
    else
      check.call []
      256.times { |a| check.call [a] }
      256.times { |a| 256.times { |b| check.call [a, b] } }
      256.times { |a| 256.times { |b| check.call [0x8f, a, b] } } if (JAPANESE.drop(2) + JIS2004).include? name
      if name.start_with? 'stateless-ISO-2022-JP'
        [0x90, 0x92].each { |prefix| 256.times { |a| 256.times { |b| check.call [prefix, a, b] } } }
      end
      if name == 'SJIS-SoftBank'
        # IO preserves legacy-looking ESC $ selectors as literal text. Include
        # every selector/payload pair with and without SI, and truncated forms.
        256.times { |a| check.call [27, 36, a] }
        256.times { |a| 256.times { |b| check.call [27, 36, a, b]; check.call [27, 36, a, b, 15] } }
      end
      if name == 'CESU-8'
        # All UTF-16 units (including lone surrogates), every valid pair, and
        # every native four-byte UTF-8 scalar. Build bytes independently of Rust.
        unit = ->(point) { [point].pack('U').bytes }
        (0..0xffff).each { |point| check.call unit.call(point) }
        (0xd800..0xdbff).each { |high| (0xdc00..0xdfff).each { |low| check.call unit.call(high) + unit.call(low) } }
        (0x10000..0x10ffff).each { |point| check.call [point].pack('U').bytes }
        [0xd800, 0xdbff].each { |high| (0..0xffff).each { |point| check.call unit.call(high) + unit.call(point) } }
        [0xdc00, 0xdfff].each { |low| (0..0xffff).each { |point| check.call unit.call(point) + unit.call(low) } }
        [[0xed,0xa0,0x80,0xed,0xb0,0x80], [0xed,0xaf,0xbf,0xed,0xbf,0xbf], [0xed,0xa0,0xbd,0xed,0xb8,0x80]].each do |base|
          base.size.times { |length| check.call base.take(length) }
          base.size.times { |index| 256.times { |byte| input = base.dup; input[index] = byte; check.call input } }
        end
        %w[f4908080 f888808080 fc8480808080 eda08041edb080 edb080eda080 eda080eda080 edb080edb080].each { |hex| check.call [hex].pack('H*').bytes }
      end
      if (UTF8_MOBILE + ['UTF8-MAC']).include? name
        # Exhaust every Unicode scalar, independently of the PUA table generator.
        (0..0x10ffff).each { |point| check.call([point].pack('U').bytes) unless point.between?(0xd800, 0xdfff) }
        [[0xc2, 0xa2], [0xe0, 0xa0, 0x80], [0xed, 0x9f, 0xbf], [0xee, 0x98, 0xbe],
         [0xee, 0x9b, 0xb1], [0xf0, 0x90, 0x80, 0x80], [0xf4, 0x8f, 0xbf, 0xbf]].each do |base|
          base.size.times { |length| check.call base.take(length) }
          base.size.times { |index| 256.times { |byte| input = base.dup; input[index] = byte; check.call input } }
        end
        check.call [0xf8, 0x88, 0x80, 0x80, 0x80]
        check.call [0xfc, 0x84, 0x80, 0x80, 0x80, 0x80]
      end
    end
    if name == 'UTF8-MAC'
      # Independent canonical candidates include Ruby exclusions and chains;
      # no expected result is inferred from the generated composition table.
      UTF8_MAC_PROBES.each do |s|
        [s, 'x' + s, s + 'y', s + "\u0301", s + "\u0323", s.reverse].each { |input| check.call input.bytes }
      end
      (0x1100..0x1112).each do |leading|
        (0x1161..0x1175).each do |vowel|
          prefix = [leading, vowel].pack('U*')
          check.call prefix.bytes
          (0x11a8..0x11c2).each do |trailing|
            check.call (prefix + [trailing].pack('U')).bytes
            syllable = 0xac00 + ((leading - 0x1100) * 21 + vowel - 0x1161) * 28
            check.call [syllable, trailing].pack('U*').bytes
          end
        end
      end
    end
    puts format('%s: 0x%016x', label, hash)
  end
  exit
end
abort 'usage: generate_encodings.rb [--fingerprints]' unless ARGV.empty?

def converter?(encoding)
  Encoding::Converter.new encoding, 'UTF-8'
  true
rescue Encoding::ConverterNotFoundError
  false
end

single_byte = Encoding.list.select do |enc|
  enc.name.match?(/\A(?:ISO-8859-|Windows-125|Windows-874|KOI8-|IBM|CP(?:850|852|855)\z|mac|TIS-620|GB1988)/) && converter?(enc)
end.sort_by(&:name)
multibyte = MULTIBYTE.map { |name| Encoding.find name }
puts "// Generated by scripts/generate_encodings.rb using Ruby #{RUBY_VERSION}, Asciidoctor #{version}."
puts '// Byte-to-Unicode mappings; u32::MAX marks a byte with no UTF-8 conversion.'
puts '// Encoding names are case-insensitive. Unknown names use the default UTF-8.'
puts 'use super::{Encoding, Iso2022Kind, MultibyteKind};'
puts 'pub(super) fn lookup(name: &str) -> Option<Encoding> {'
puts '    Some(match name {'
Encoding.list.sort_by(&:name).each do |enc|
  names = enc.names.map(&:downcase).uniq.sort
  kind = case enc.name
         when 'UTF-8' then 'Encoding::Utf8'
         when 'CESU-8' then 'Encoding::Cesu8'
         when 'UTF8-MAC' then 'Encoding::Utf8Mac'
         when 'ASCII-8BIT' then 'Encoding::Binary'
         when 'US-ASCII' then 'Encoding::Ascii'
         when 'UTF-16' then 'Encoding::Utf16(None)'
         when 'UTF-16LE' then 'Encoding::Utf16(Some(true))'
         when 'UTF-16BE' then 'Encoding::Utf16(Some(false))'
         when 'UTF-32' then 'Encoding::Utf32(None)'
         when 'UTF-32LE' then 'Encoding::Utf32(Some(true))'
         when 'UTF-32BE' then 'Encoding::Utf32(Some(false))'
         when 'ISO-2022-JP' then "Encoding::Iso2022Jp { table: &MULTI_#{MULTIBYTE.index('EUC-JP')}, kind: Iso2022Kind::Standard }"
         when 'CP50220', 'CP50221' then "Encoding::Iso2022Jp { table: &MULTI_#{MULTIBYTE.index('CP51932')}, kind: Iso2022Kind::Windows }"
         when 'ISO-2022-JP-KDDI' then "Encoding::Iso2022Jp { table: &MULTI_#{MULTIBYTE.index('CP51932')}, kind: Iso2022Kind::Kddi }"
         when 'stateless-ISO-2022-JP' then "Encoding::Multibyte { table: &MULTI_#{MULTIBYTE.index('EUC-JP')}, kind: MultibyteKind::StatelessIso2022Jp }"
         when 'stateless-ISO-2022-JP-KDDI' then "Encoding::Multibyte { table: &MULTI_#{MULTIBYTE.index('CP51932')}, kind: MultibyteKind::StatelessIso2022JpKddi }"
         when 'SJIS-KDDI' then "Encoding::Multibyte { table: &MULTI_#{MULTIBYTE.index('Windows-31J')}, kind: MultibyteKind::ShiftJisKddi }"
         when 'SJIS-DoCoMo' then "Encoding::Multibyte { table: &MULTI_#{MULTIBYTE.index('Windows-31J')}, kind: MultibyteKind::ShiftJisDocomo }"
         when 'SJIS-SoftBank' then "Encoding::Multibyte { table: &MULTI_#{MULTIBYTE.index('Windows-31J')}, kind: MultibyteKind::ShiftJisSoftbank }"
         when *UTF8_MOBILE then "Encoding::Utf8Mobile(&#{enc.name.upcase.tr('-', '_')}_OVERRIDES)"
         else
           if (index = single_byte.index(enc))
             "Encoding::Single(&TABLE_#{index})"
           elsif (index = multibyte.index(enc))
             family = if index < 2
                        'ShiftJis'
                      elsif index < JAPANESE.size || JIS2004.include?(enc.name)
                        'EucJp'
                      elsif BIG5.include? enc.name
                        'Big5'
                      elsif KOREAN.include? enc.name
                        'Korean'
                      else
                        { 'GB2312' => 'Gb2312', 'GB12345' => 'Gb2312', 'GBK' => 'Gbk', 'GB18030' => 'Gb18030' }.fetch(enc.name)
                      end
             "Encoding::Multibyte { table: &MULTI_#{index}, kind: MultibyteKind::#{family} }"
           elsif converter?(enc)
             abort "unimplemented converter in pinned Ruby: #{enc.name}"
           else
             'Encoding::Unavailable'
           end
         end
  puts "        #{names.map(&:inspect).join(' | ')} => #{kind},"
end
puts '        _ => return None,'
puts '    })'
puts '}'
cp51932 = {}
windows31j = {}
multibyte.each_with_index do |enc, index|
  # Enumerate byte sequences, not encoded Unicode: the reverse mapping loses
  # duplicate characters and vendor-specific differences. Only one-character
  # conversions belong in the table. ASCII is handled directly by the decoder.
  candidates = (128..255).map { |byte| [byte] }
  if index < 2
    leads = (0x81..0x9f).to_a + (0xe0..0xfc).to_a
    trails = (0x40..0x7e).to_a + (0x80..0xfc).to_a
    candidates.concat leads.product(trails)
  elsif index < JAPANESE.size || JIS2004.include?(enc.name)
    candidates.concat [0x8e].product((0xa1..0xdf).to_a)
    candidates.concat (0xa1..0xfe).to_a.product((0xa1..0xfe).to_a)
    candidates.concat [0x8f].product((0xa1..0xfe).to_a, (0xa1..0xfe).to_a)
  elsif BIG5.include? enc.name
    candidates.concat (0x81..0xfe).to_a.product((0x40..0x7e).to_a + (0xa1..0xfe).to_a)
  elsif %w[GB2312 GB12345].include? enc.name
    candidates.concat (0xa1..0xfe).to_a.product((0xa1..0xfe).to_a)
  elsif KOREAN.include? enc.name
    candidates.concat (0x81..0xfe).to_a.product((0x00..0xff).to_a)
  else
    candidates.concat (0x81..0xfe).to_a.product((0x40..0x7e).to_a + (0x80..0xfe).to_a)
  end
  values = candidates.filter_map do |bytes|
    begin
      points = bytes.pack('C*').force_encoding(enc).encode('UTF-8').codepoints
      raise "multi-character mapping: #{enc.name}/#{bytes.inspect}" unless points.size == 1
      key = bytes.reduce(0) { |value, byte| (value << 8) | byte }
      (key << 32) | points.first
    rescue EncodingError
      nil
    end
  end.sort
  cp51932 = values.to_h { |value| [value >> 32, [value & 0xffffffff].pack('U')] } if enc.name == 'CP51932'
  windows31j = values.to_h { |value| [value >> 32, [value & 0xffffffff].pack('U')] } if enc.name == 'Windows-31J'
  puts "// #{enc.name}: sorted (encoded bytes << 32) | Unicode scalar."
  puts '#[rustfmt::skip]'
  puts "static MULTI_#{index}: [u64; #{values.size}] = ["
  values.each_slice(4) do |row|
    puts "    #{row.map { |value| format('0x%016x', value) }.join(', ')},"
  end
  puts '];'
end

# Reuse the existing CP51932 scalar table, recording every defined, compound
# or explicitly undefined difference from Ruby's actual KDDI decoder. Reverse
# UTF-8 encoding is asymmetric and would invent emoji mappings Ruby rejects.
overrides = []
256.times do |a|
  256.times do |b|
    value = begin
      converter = Encoding::Converter.new 'stateless-ISO-2022-JP-KDDI', 'UTF-8'
      converter.convert([0x92, a, b].pack('C*')) + converter.finish
    rescue EncodingError
      nil
    end
    key = (a << 8) | b
    overrides << [key, value] if value != cp51932[key]
  end
end
puts '// KDDI: sorted EUC byte pairs overriding CP51932; None rejects a base mapping.'
puts '#[rustfmt::skip]'
puts "pub(super) static KDDI_OVERRIDES: [(u16, Option<&str>); #{overrides.size}] = ["
overrides.each do |key, value|
  literal = value ? 'Some("' + value.codepoints.map { |point| "\\u{#{point.to_s(16)}}" }.join + '")' : 'None'
  puts format('    (0x%04x, %s),', key, literal)
end
puts '];'

# Mobile SJIS variants share Windows-31J's grammar, with their own mappings.
# Enumerate external characters, not reverse UTF-8 encoding: undefined emoji
# and retained private-use characters must stay exactly as the IO converter.
sjis_candidates = (128..255).map { |byte| [byte] }
((0x81..0x9f).to_a + (0xe0..0xfc).to_a).each do |lead|
  256.times { |trail| sjis_candidates << [lead, trail] }
end
SJIS_MOBILE.each do |name|
  sjis_overrides = sjis_candidates.filter_map do |bytes|
    value = begin
      converter = Encoding::Converter.new name, 'UTF-8'
      converter.convert(bytes.pack('C*')) + converter.finish
    rescue EncodingError
      nil
    end
    key = bytes.reduce(0) { |result, byte| (result << 8) | byte }
    [key, value] if value != windows31j[key]
  end.sort_by(&:first)
  puts "// #{name}: sorted byte keys overriding Windows-31J; None rejects a base mapping."
  puts '#[rustfmt::skip]'
  puts "pub(super) static #{name.upcase.tr('-', '_')}_OVERRIDES: [(u16, Option<&str>); #{sjis_overrides.size}] = ["
  sjis_overrides.each do |key, value|
    literal = value ? 'Some("' + value.codepoints.map { |point| "\\u{#{point.to_s(16)}}" }.join + '")' : 'None'
    puts format('    (0x%04x, %s),', key, literal)
  end
  puts '];'
end

# UTF8 mobile variants use Unicode scalar keys, separate from encoded SJIS keys.
UTF8_MOBILE.each do |name|
  overrides = (0xe000..0xf8ff).filter_map do |point|
    input = [point].pack('U')
    value = begin
      converter = Encoding::Converter.new name, 'UTF-8'
      converter.convert(input) + converter.finish
    rescue EncodingError
      nil
    end
    [point, value] if value != input
  end
  puts "// #{name}: Unicode scalar overrides; None rejects a private-use value."
  puts '#[rustfmt::skip]'
  puts "pub(super) static #{name.upcase.tr('-', '_')}_OVERRIDES: [(u32, Option<&str>); #{overrides.size}] = ["
  overrides.each do |key, value|
    literal = value ? 'Some("' + value.codepoints.map { |point| "\\u{#{point.to_s(16)}}" }.join + '")' : 'None'
    puts format('    (0x%04x, %s),', key, literal)
  end
  puts '];'
end

# Unicode's canonical pairs are candidates only: the installed Ruby IO
# converter selects its historical MAC subset. Hangul is handled algorithmically.
mac_pairs = UnicodeNormalize::COMPOSITION_TABLE.keys.filter_map do |input|
  converter = Encoding::Converter.new 'UTF8-MAC', 'UTF-8'
  value = converter.convert(input) + converter.finish
  next if value == input
  points = input.codepoints
  raise "unexpected MAC composition: #{input.inspect}" unless points.size == 2 && value.codepoints.size == 1 && value.ord <= 0xffff
  [(points[0] << 32) | points[1], value.ord]
end.sort
puts '// UTF8-MAC: adjacent canonical pair compositions verified by strict Ruby IO.'
puts '#[rustfmt::skip]'
puts "pub(super) static UTF8_MAC_COMPOSITIONS: [(u64, u32); #{mac_pairs.size}] = ["
mac_pairs.each { |key, value| puts format('    (0x%016x, 0x%04x),', key, value) }
puts '];'
puts '// Independent canonical inputs, including exclusions; not derived from MAC mappings.'
puts '#[cfg(test)]'
puts '#[rustfmt::skip]'
puts "pub(super) static UTF8_MAC_PROBES: [&str; #{UTF8_MAC_PROBES.size}] = ["
UTF8_MAC_PROBES.each do |input|
  puts '    "' + input.codepoints.map { |point| "\\u{#{point.to_s(16)}}" }.join + '",'
end
puts '];'

# Compact only consecutive, verified mappings. Gaps stay explicit, including
# pointers outside Unicode and the unused interval before supplementary planes.
ranges = []
(126 * 10 * 126 * 10).times do |pointer|
  rest, d = pointer.divmod(10)
  rest, c = rest.divmod(126)
  a, b = rest.divmod(10)
  begin
    points = [a + 0x81, b + 0x30, c + 0x81, d + 0x30].pack('C*').force_encoding('GB18030').encode('UTF-8').codepoints
    raise "multi-character GB18030 mapping: #{pointer}" unless points.size == 1
    scalar = points.first
    last = ranges[-1]
    if last && pointer == last[1] + 1 && scalar == last[2] + pointer - last[0]
      last[1] = pointer
    else
      ranges << [pointer, pointer, scalar]
    end
  rescue EncodingError
    nil
  end
end
puts '// GB18030: inclusive pointer start/end and first Unicode scalar.'
puts '#[rustfmt::skip]'
puts "pub(super) static GB18030_RANGES: [(u32, u32, u32); #{ranges.size}] = ["
ranges.each { |first, last, scalar| puts format('    (%d, %d, 0x%x),', first, last, scalar) }
puts '];'

single_byte.each_with_index do |enc, index|
  values = (0..255).map do |byte|
    begin
      points = [byte].pack('C').force_encoding(enc).encode('UTF-8').codepoints
      raise "multi-character mapping: #{enc.name}/#{byte}" unless points.size == 1
      points.first
    rescue EncodingError
      nil
    end
  end
  # All supported single-byte sets except EBCDIC preserve the ASCII prefix.
  values = values.drop(128) if values.take(128) == (0..127).to_a
  puts "// #{enc.name}"
  puts "static TABLE_#{index}: [u32; #{values.size}] = ["
  values.each_slice(8) do |row|
    puts "    #{row.map { |v| v ? format('0x%04x', v) : 'u32::MAX' }.join(', ')},"
  end
  puts '];'
end
