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
  'implicit xrefs borrow formatted titles without HTML' => lambda do
    node = project("See <<s>>.\n\n[#s]\n== A *bold* title")[:blocks][0][:inlines][1]
    assert(node[:target] == '#s' && node[:inlines][0][:value] == 'A ', 'missing generated label')
    assert(node[:inlines][1][:kind] == 'strong' && node[:inlines][1][:inlines][0][:value] == 'bold', 'generated label flattened')
  end,
  'xref labels strip nested links and stop recursive references' => lambda do
    node = project("See <<s>>.\n\n[#s]\n== A https://example.org[link] and <<s>>")[:blocks][0][:inlines][1]
    assert(node[:inlines].all? { |part| part[:kind] == 'text' }, 'nested anchor survived')
    assert(node[:inlines].map { |part| part[:value] }.join == 'A link and [s]', 'recursive fallback changed')
  end,
  'xref root external and missing targets use distinct fallbacks' => lambda do
    nodes = project('xref:#[] xref:other.adoc#part[] xref:missing[]')[:blocks][0][:inlines]
    assert(nodes[0][:inlines] == [{ kind: 'text', value: '[^top]' }], 'root fallback changed')
    assert(nodes[2][:target] == 'other.html#part' && nodes[2][:inlines] == [{ kind: 'text', value: 'other.html' }], 'external path fallback changed')
    assert(nodes[4][:inlines] == [{ kind: 'text', value: '[missing]' }], 'unknown target fallback changed')
  end,
  'xrefstyle full preserves caption number and typographic quotes' => lambda do
    node = project(":xrefstyle: full\n\n<<ex>>\n\n[#ex]\n.A *sample*\n====\nContent.\n====")[:blocks][0][:inlines][0]
    assert(node[:inlines][0][:value] == 'Example 1, &#8220;A ', 'caption prefix changed')
    assert(node[:inlines][1][:kind] == 'strong' && node[:inlines][-1][:value] == '&#8221;', 'caption formatting changed')
  end,
  'attribute xrefs freeze labels at definition time in snapshots' => lambda do
    tree = project(":early: pass:m[xref:s[]]\n\n[[s]]\n== Target *Ж*\n\n:late: pass:m[xref:s[]]\n\n{early} {late}", ['early', 'late'])
    assert(tree[:attributes][:header]['early'][0][:inlines] == [{ kind: 'text', value: '[s]' }], 'forward definition borrowed a later title')
    assert(tree[:attributes][:header]['late'].nil?, 'body definition leaked to header')
    assert(tree[:attributes][:final]['late'][0][:inlines][1][:kind] == 'strong', 'final snapshot lost label formatting')
  end,
  'indexterms keep visibility hierarchy see and see-also independently' => lambda do
    nodes = project('((*Visible*)) (((Animals,Cats))) indexterm2:[Entry,see-also="First,Second"] indexterm:[Top,Leaf,see=Other]')[:blocks][0][:inlines]
    assert(nodes[0][:kind] == 'indexterm' && nodes[0][:inlines][0][:kind] == 'strong', 'visible term flattened')
    assert(nodes[2][:inlines].nil? && nodes[2][:terms] == ['Animals', 'Cats'], 'concealed hierarchy lost')
    assert(nodes[4][:see_also] == ['First', 'Second'], 'see-also order lost')
    assert(nodes[6][:terms] == ['Top', 'Leaf'] && nodes[6][:see] == 'Other', 'see target lost')
  end,
  'inline and block formulas preserve notation content and whitespace' => lambda do
    nodes = project(":stem: latexmath\n\nstem:[x_1] asciimath:[a  +\n  b]")[:blocks][0][:inlines]
    assert(nodes[0][:kind] == 'stem' && nodes[0][:style] == 'latexmath', 'stem alias lost')
    assert(nodes[2][:style] == 'asciimath' && nodes[2][:inlines] == [{ kind: 'text', value: "a  +\n  b" }], 'formula whitespace folded')
    block = project("[asciimath]\n++++\na  < b\n  + c\n++++")[:blocks][0]
    assert(block[:style] == 'asciimath' && block[:inlines] == [{ kind: 'text', value: "a  &lt; b\n  + c" }], 'block formula content changed')
  end,
  'typographic quotes preserve span metadata and nested formatting' => lambda do
    node = project('[.quoted]"`A *word*`"')[:blocks][0][:inlines][0]
    assert(node[:kind] == 'span' && node[:roles] == ['quoted'], 'quote metadata lost')
    assert(node[:inlines][0][:value] == '&#8220;A ' && node[:inlines][1][:kind] == 'strong' && node[:inlines][-1][:value] == '&#8221;', 'quote text or formatting lost')
  end,
  'tables retain columns header footer spans and alignment' => lambda do
    node = project(".Table *title*\n[#tab.wide,cols=\"1,2\",options=\"header,footer\"]\n|===\n|Name |Value\n2+^.^|*Wide*\n|Last |End\n|===")[:blocks][0]
    assert(node[:id] == 'tab' && node[:roles] == ['wide'] && node[:title][1][:kind] == 'strong', 'table metadata lost')
    assert(node[:columns].map { |col| col[:width] } == ['33.3333', '66.6667'], 'column widths lost')
    assert(node[:head][0][0][:inlines] == [{ kind: 'text', value: 'Name' }], 'header text lost')
    cell = node[:body][0][0]
    assert(cell[:colspan] == 2 && cell[:rowspan] == 1 && cell[:halign] == 'center' && cell[:valign] == 'middle', 'cell layout lost')
    assert(cell[:paragraphs][0][0][:kind] == 'strong' && node[:foot][0][0][:paragraphs][0][0][:value] == 'Last', 'cell contents lost')
  end,
  'cell styles remain distinct from explicit inline formatting' => lambda do
    node = project("[cols=\"s,l\",options=noheader]\n|===\n|*Bold*\n\nSecond.\nl|  *literal* < &\n    next\n|===")[:blocks][0]
    cell = node[:body][0][0]
    assert(cell[:style] == 'strong' && cell[:paragraphs].size == 2, 'style or paragraph boundaries lost')
    assert(cell[:paragraphs][0][0][:kind] == 'strong' && cell[:paragraphs][1] == [{ kind: 'text', value: 'Second.' }], 'invented style wrapper')
    assert(node[:body][0][1][:literal] == [{ kind: 'text', value: "  *literal* &lt; &amp;\n    next" }], 'literal whitespace or escaping changed')
  end,
  'CSV literal cells keep normal substitutions and significant newlines' => lambda do
    cell = project("[format=csv,cols=\"l,1\",options=noheader]\n|===\n\"*Ж* < 2\n    next\",value\n|===")[:blocks][0][:body][0][0]
    assert(cell[:literal][0][:kind] == 'strong', 'CSV substitutions disabled')
    assert(cell[:literal][1][:value] == " &lt; 2\n    next", 'CSV literal newline folded')
  end,
  'AsciiDoc cells convert nested documents without using HTML' => lambda do
    node = project("[cols=a,options=noheader]\n|===\n|* Item\n\n[cols=\"1,1\",options=header]\n!===\n!Key !Value\n!A !*B*\n!===\n|===")[:blocks][0]
    nested = node[:body][0][0][:document]
    assert(nested[:title].nil? && nested[:blocks][0][:context] == 'ulist', 'nested document/list lost')
    assert(nested[:blocks][1][:body][0][1][:paragraphs][0][0][:kind] == 'strong', 'nested table formatting lost')
  end,
  'cell substitutions execute once in header body footer order' => lambda do
    result = project(":n: 0\n\n[cols=\"1,1\",options=\"header,footer\"]\n|===\n|{counter:n} |H\n|{counter:n} |{set:cellbgcolor:red}Color\n|{counter:n} |End\n|===", ['n'])
    node = result[:blocks][0]
    assert(node[:head][0][0][:inlines][0][:value] == '1', 'header counter changed')
    assert(node[:body][0][0][:paragraphs][0][0][:value] == '2', 'body counter changed')
    assert(node[:foot][0][0][:paragraphs][0][0][:value] == '3', 'footer counter changed')
    assert(node[:body][0][0][:background].nil? && node[:body][0][1][:background] == 'red' && node[:foot][0][0][:background] == 'red', 'background state lost')
    assert(result[:attributes][:final]['n'] == [{ kind: 'text', value: '3' }], 'cell substitutions executed twice')
  end,
  'empty headers have no inline text and empty body cells have no paragraphs' => lambda do
    node = project("[cols=\"1,1\",options=header]\n|===\n| |Title\n| |Value\n|===")[:blocks][0]
    assert(node[:head][0][0][:inlines] == [], 'empty header gained content')
    assert(node[:body][0][0][:paragraphs] == [], 'empty body cell gained a paragraph')
  end,
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
