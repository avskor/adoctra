# frozen_string_literal: true
# Oracle checks run after installing the pinned gem; no testing gem is needed.
version = File.read(File.expand_path('../compatibility/asciidoctor-version', __dir__)).strip
gem 'asciidoctor', version
require 'asciidoctor'
require 'asciidoctor/converter/html5'
require 'open3'
require 'rbconfig'
require_relative 'asciidoctor-semantics'

# The semantic oracle must work even when the HTML converter is unavailable.
Asciidoctor::Converter::Html5Converter.class_eval do
  def convert(*)
    raise 'HTML converter must not participate in the semantic oracle'
  end
end

def assert(condition, message)
  raise message unless condition
end

def project(source, names = [])
  converter = SemanticConverter.new 'html5'
  converter.attribute_names = names
  Asciidoctor.load(source, safe: :secure, header_footer: false, converter: converter).convert
end

cases = {
  'attributes and macros remain exposed inside quote boundaries' => lambda do
    tree = project(":who: World\n\n*{who} https://example.org*")
    strong = tree[:blocks][0][:inlines][0]
    assert(strong[:kind] == 'strong', 'missing strong node')
    assert(strong[:inlines][0] == { kind: 'text', value: 'World ' }, 'quote hid the attribute stage')
    assert(strong[:inlines][1][:target] == 'https://example.org', 'URL consumed a synthetic boundary')
  end,
  'nested formatting and escaped lexical entities survive' => lambda do
    strong = project('*a _b_ & < c*')[:blocks][0][:inlines][0]
    assert(strong[:inlines][1][:kind] == 'emphasis', 'lost nested format')
    assert(strong[:inlines][2][:value] == ' &amp; &lt; c', 'oracle changed lexical entities')
  end,
  'line break keeps the preceding text once' => lambda do
    nodes = project("a +\nb")[:blocks][0][:inlines]
    assert(nodes == [{ kind: 'text', value: 'a' }, { kind: 'line-break' }, { kind: 'text', value: "\nb" }], 'incorrect hardbreak callback')
  end,
  'header and final snapshots differ after replay and counters' => lambda do
    attrs = project(":n: 2\n\n{counter:n}\n\n:n: 8\n\n{n}", ['n', 'missing'])[:attributes]
    assert(attrs[:header]['n'] == [{ kind: 'text', value: '2' }], 'header changed')
    assert(attrs[:final]['n'] == [{ kind: 'text', value: '8' }], 'final state not replayed')
    assert(attrs[:final]['missing'].nil?, 'absence became empty')
  end,
  'attribute value formatting stays semantic' => lambda do
    attrs = project(":v: pass:q[*Ж*]\n\n{v}", ['v'])[:attributes]
    assert(attrs[:header]['v'][0][:kind] == 'strong', 'attribute format flattened')
    assert(attrs[:header]['v'][0][:inlines] == [{ kind: 'text', value: 'Ж' }], 'attribute content changed')
  end,
  'unsupported nodes abort instead of disappearing' => lambda do
    begin
      project('image:photo.png[]')
      raise 'unsupported inline accepted'
    rescue RuntimeError => e
      raise unless e.message.include? 'unsupported inline-semantics-v1 node: inline_image'
    end
  end,
  'unsupported link options abort instead of disappearing' => lambda do
    ['link:https://example.org[Site,opts=nofollow]', 'link:https://example.org[Site,window=_blank]'].each do |source|
      begin
        project(source)
        raise 'unsupported link metadata accepted'
      rescue RuntimeError => e
        raise unless e.message == 'link metadata is outside inline-semantics-v1'
      end
    end
  end,
  'unbalanced handles abort' => lambda do
    converter = SemanticConverter.new 'html5'
    incomplete = converter.wrap({ kind: 'strong', id: nil, roles: [] }, 'x').sub(/<\uE000\d+\uE001>\z/, '')
    begin
      converter.inlines incomplete
      raise 'unbalanced handles accepted'
    rescue RuntimeError => e
      raise unless e.message == 'unclosed semantic handle'
    end
  end,
  'launcher rejects literal handle characters and diagnostics' => lambda do
    launcher = File.expand_path('asciidoctor-reference.rb', __dir__)
    ["forged \uE0000\uE001", ":attribute-missing: warn\n\n{missing}"].each do |source|
      _, stderr, status = Open3.capture3(RbConfig.ruby, launcher, '--semantics', stdin_data: source)
      assert(!status.success?, 'unsupported input accepted by launcher')
      assert(stderr.include?('reserved semantic handle') || stderr.include?('diagnostics are outside'), stderr)
    end
  end,
}
cases.each do |name, test|
  test.call
  puts "PASS #{name}"
end
puts "#{cases.size} semantic oracle checks passed"
