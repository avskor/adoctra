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
      project('footnote:[text]')
      raise 'unsupported inline accepted'
    rescue RuntimeError => e
      raise unless e.message.include? 'unsupported inline-semantics-v1 node: inline_footnote'
    end
  end,
  'unsupported link options abort instead of disappearing' => lambda do
    ['link:https://example.org[Site,opts=custom]', 'link:https://example.org[Site,custom=value]'].each do |source|
      begin
        project(source)
        raise 'unsupported link metadata accepted'
      rescue RuntimeError => e
        raise unless e.message == 'link metadata is outside inline-semantics-v1'
      end
    end
  end,
  'link metadata retains declarations and derived relation' => lambda do
    link = project('link:https://example.org[*Site*,id=home,role="primary wide",title="A & B",window=_blank,opts=nofollow]')[:blocks][0][:inlines][0]
    assert(link[:metadata] == { id: 'home', role: 'primary wide', title: 'A &amp; B', window: '_blank', rel: 'nofollow noopener' }, 'lost link metadata')
    assert(link[:inlines][0][:kind] == 'strong', 'lost link label formatting')
    named = project('link:https://example.org[Site,window=work]')[:blocks][0][:inlines][0]
    assert(named[:metadata] == { window: 'work' }, 'invented noopener on named window')
  end,
  'UI labels retain formatting and key order through attributes' => lambda do
    tree = project(":experimental:\n:label: pass:q[*Save*]\n:shortcut: pass:m[kbd:[Ctrl+S]]\n\nbtn:[{label}] {shortcut}", ['shortcut'])
    nodes = tree[:blocks][0][:inlines]
    assert(nodes[0][:kind] == 'button' && nodes[0][:inlines][0][:kind] == 'strong', 'button label flattened')
    keys = [[{ kind: 'text', value: 'Ctrl' }], [{ kind: 'text', value: 'S' }]]
    assert(nodes[2] == { kind: 'kbd', keys: keys }, 'keyboard keys changed')
    assert(tree[:attributes][:header]['shortcut'] == [nodes[2]], 'attribute lost keyboard semantics')
  end,
  'menu reference and path have distinct structure' => lambda do
    nodes = project(":experimental:\n\nmenu:File[] menu:File[New > Project]")[:blocks][0][:inlines]
    assert(nodes[0][:item].nil? && nodes[0][:submenus] == [], 'bare menu gained an item')
    assert(nodes[2][:menu] == [{ kind: 'text', value: 'File' }], 'lost menu name')
    assert(nodes[2][:submenus] == [[{ kind: 'text', value: 'New' }]], 'lost submenu')
    assert(nodes[2][:item] == [{ kind: 'text', value: 'Project' }], 'lost menu item')
  end,
  'image id attributes and formatted alt survive independently of HTML' => lambda do
    node = project(":imagesdir: assets\n\nimage:cat.png[alt='*Cat* & friend',width=24,id=cat,role=zoom]")[:blocks][0][:inlines][0]
    assert(node[:id] == 'cat' && node[:roles] == ['zoom'], 'inline image id/role lost')
    assert(node[:target] == 'cat.png' && node[:src] == 'assets/cat.png', 'image URI lost')
    assert(node[:attributes] == { 'width' => '24' }, 'image attributes changed')
    assert(node[:alt][0][:kind] == 'strong' && node[:alt][1][:value] == ' &amp; friend', 'image alt flattened')
  end,
  'block image accepts caption replay bookkeeping without exporting it' => lambda do
    node = project(".Picture\n[#photo.zoom]\nimage::cat.png[Cat,24,16]")[:blocks][0]
    assert(node[:context] == 'image' && node[:id] == 'photo', 'block image metadata lost')
    assert(node[:title] == [{ kind: 'text', value: 'Picture' }], 'image title changed')
    assert(node[:attributes] == { 'width' => '24', 'height' => '16' }, 'internal attribute entries leaked')
  end,
  'list items preserve checkboxes attached blocks and absent descriptions' => lambda do
    blocks = project("* [x] Done\n+\nAttached.\n\n* [ ] Pending")[:blocks]
    assert(blocks[0][:items].map { |entry| entry[:checkbox] } == [true, false], 'checkbox state lost')
    assert(blocks[0][:items][0][:blocks][0][:inlines] == [{ kind: 'text', value: 'Attached.' }], 'attached block lost')
    entries = project("Term:: Description\nLast::")[:blocks][0][:items]
    assert(entries[0][:terms] == [[{ kind: 'text', value: 'Term' }]], 'description term lost')
    assert(entries[1][:description].nil?, 'absent description became empty')
  end,
  'verbatim substitutions preserve whitespace and container content model' => lambda do
    blocks = project("NOTE: Simple.\n\n[WARNING]\n====\nParagraph.\n====\n\n[source,ruby,subs=\"verbatim,quotes\"]\n----\n  *x* < 2\n    y\n----")[:blocks]
    assert(blocks[0][:inlines] == [{ kind: 'text', value: 'Simple.' }] && !blocks[0].key?(:blocks), 'simple content changed')
    assert(blocks[1][:type] == 'warning' && blocks[1][:blocks][0][:context] == 'paragraph', 'compound content changed')
    code = blocks[2]
    assert(code[:language] == 'ruby', 'listing language lost')
    assert(code[:inlines][0][:value] == '  ' && code[:inlines][1][:kind] == 'strong' && code[:inlines][2][:value] == " &lt; 2\n    y", 'verbatim content changed')
  end,
  'image options and icons fail closed' => lambda do
    ['icon:heart[]', 'image:photo.svg[opts=inline]', 'image:photo.png[custom=value]'].each do |source|
      begin
        project(source)
        raise 'unsupported image accepted'
      rescue RuntimeError => e
        raise unless e.message.include? 'outside inline-semantics-v1'
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
