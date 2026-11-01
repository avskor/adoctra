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
  'footnote markers and ordered catalog survive repeated references' => lambda do
    tree = project('First footnote:n[A *note*]. Again footnote:n[] footnote:n[Ignored].')
    markers = tree[:blocks][0][:inlines].select { |n| n[:kind] == 'footnote' }
    assert(markers.map { |n| n[:type] } == ['definition', 'reference', 'reference'], 'marker types lost')
    assert(markers.map { |n| n[:index] } == ['1', '1', '1'], 'reference number changed')
    assert(tree[:footnotes].size == 1 && tree[:footnotes][0][:id] == 'n', 'catalog duplicated')
    assert(tree[:footnotes][0][:inlines][1][:kind] == 'strong', 'definition formatting lost')
  end,
  'unused removed and suppressed footnotes remain cataloged' => lambda do
    tree = project(":note: pass:q,m[footnote:n[Stored *note*]]\n:nofootnotes:\n\n:note!:\n\nBody.")
    assert(tree[:footnotes].size == 1 && tree[:footnotes][0][:id] == 'n', 'unused definition missing')
    assert(tree[:footnotes][0][:inlines][1][:kind] == 'strong', 'unused definition flattened')
    assert(tree[:blocks][0][:inlines] == [{ kind: 'text', value: 'Body.' }], 'invented visible marker')
  end,
  'footnote catalogs preserve duplicate counter values and nested ownership' => lambda do
    tree = project("Outer footnote:o[Outer].\n\n[cols=a,options=noheader]\n|===\n|:note: pass:m[footnote:i[Inner]]\n\nCell.\n|===\n\nAfter footnote:[Last].")
    assert(tree[:footnotes].map { |n| n[:index] } == ['1', '2'], 'root counter changed')
    nested = tree[:blocks][1][:body][0][0][:document]
    assert(nested[:footnotes].map { |n| [n[:index], n[:id]] } == [['1', 'i']], 'nested catalog leaked')
    reset = project("footnote:a[One]\n\n:footnote-number: 0\n\nfootnote:b[Two]")
    assert(reset[:footnotes].map { |n| n[:index] } == ['1', '1'], 'duplicate indices collapsed')
  end,
  'xref borrowed footnotes strip anchors but keep visible superscript' => lambda do
    tree = project("See <<s>>.\n\n[#s]\n== Title footnote:n[Note]")
    label = tree[:blocks][0][:inlines][1][:inlines]
    assert(label[-1] == { kind: 'superscript', id: '_footnote_n', roles: ['footnote'], inlines: [{ kind: 'text', value: '[1]' }] }, 'borrowed footnote marker changed')
    assert(tree[:footnotes].size == 1, 'borrowed marker registered twice')
  end,
  'callouts keep guards numbering and list bindings without parsing HTML' => lambda do
    tree = project("----\nЖ // <.>\nx <!--2-->\nagain <1>\n----\n<1> First.\n<2> Second.")
    marks = tree[:blocks][0][:inlines].select { |n| n[:kind] == 'callout' }
    assert(marks.map { |n| n[:number] } == ['1', '2', '1'], 'automatic numbering lost')
    assert(marks[0][:guard] == '// ' && !marks[0][:xml] && marks[1][:xml] && marks[1][:guard].nil?, 'guards lost')
    items = tree[:blocks][1][:items]
    assert(items[0][:coids] == [marks[0][:id], marks[2][:id]] && items[1][:coids] == [marks[1][:id]], 'list binding lost')
  end,
  'escaped callouts remain text and custom substitutions retain formatting' => lambda do
    tree = project("[subs=\"verbatim,quotes\"]\n----\nЖ \\<1>\n*bold* <1>\n----\n<1> Explanation.")
    nodes = tree[:blocks][0][:inlines]
    assert(nodes[0][:value] == "Ж &lt;1&gt;\n", 'escaped mark consumed')
    assert(nodes[1][:kind] == 'strong' && nodes[-1][:kind] == 'callout', 'custom substitutions lost')
  end,
  'callout lists capture icon mode before their text converts' => lambda do
    converter = SemanticConverter.new 'html5'
    converter.attribute_names = []
    source = ":icons:\n:iconsdir: assets/icons\n\n----\nx <1>\ny <2>\n----\n<1> First.\n<2> Second."
    tree = Asciidoctor.load(source, safe: :safe, header_footer: false, converter: converter).convert
    marks = tree[:blocks][0][:inlines].select { |n| n[:kind] == 'callout' }
    assert(marks.map { |n| n[:src] } == ['assets/icons/callouts/1.png', 'assets/icons/callouts/2.png'], 'image icon paths lost')
    assert(tree[:blocks][1][:items].map { |n| n[:icons] } == ['', ''], 'image mode lost')
    font = Asciidoctor.load(source.sub(':icons:', ':icons: font'), safe: :safe, header_footer: false, converter: SemanticConverter.new('html5')).convert
    assert(font[:blocks][1][:items][0][:icons] == 'font' && font[:blocks][1][:items][0][:src] == '', 'font mode changed')
  end,
  'successive callout lists have independent ids and attached footnotes' => lambda do
    tree = project("----\nx <1>\n----\n<1> First footnote:[Note].\n+\nAttached.\n\n----\ny <1>\n----\n<1> Second.")
    first, second = tree[:blocks][1][:items][0], tree[:blocks][3][:items][0]
    assert(first[:coids] == ['CO1-1'] && second[:coids] == ['CO2-1'], 'list ids reused')
    assert(first[:blocks][0][:inlines][0][:value] == 'Attached.' && tree[:footnotes].size == 1, 'attached content lost')
  end,
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
  'tables retain caption frame grid stripes width and float' => lambda do
    node = project(":table-stripes: odd\n\n.T\n[%autowidth,frame=topbot,grid=rows,width=150%,float=left]\n|===\n|a\n|===")[:blocks][0]
    assert(node[:caption] == [{ kind: 'text', value: 'Table 1. ' }], 'caption lost')
    assert(node[:frame] == 'ends' && node[:grid] == 'rows' && node[:stripes] == 'odd', 'frame/grid/stripes lost')
    assert(node[:width] == 100 && node[:explicit_width] && node[:autowidth] && node[:float] == 'left', 'width/float lost')
    node = project("|===\n|a\n|===")[:blocks][0]
    assert(node[:caption].nil? && node[:frame] == 'all' && node[:stripes].nil? && !node[:explicit_width], 'table defaults changed')
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
      project('video::clip.mp4[]')
      raise 'unsupported inline accepted'
    rescue RuntimeError => e
      raise unless e.message.include? 'unsupported inline-semantics-v1 node: video'
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
  'image align is a compared attribute' => lambda do
    tree = project('image::cat.png[Cat,align=center]')
    assert(tree[:blocks][0][:attributes] == { 'align' => 'center' }, 'image align lost')
  end,
  'quote and verse keep credit and verse whitespace' => lambda do
    tree = project("[quote,'*Ann*',Book]\n____\nQ.\n\nR.\n____\n\n[verse,Bob]\n____\n  one\n    two\n____\n\n[quote]\nOnly.")
    quote, verse, simple = tree[:blocks]
    assert(quote[:attribution][0][:kind] == 'strong' && quote[:citetitle] == [{ kind: 'text', value: 'Book' }], 'quote credit lost')
    assert(quote[:blocks].size == 2, 'compound quote flattened')
    assert(verse[:attribution] == [{ kind: 'text', value: 'Bob' }] && verse[:citetitle].nil?, 'verse credit lost')
    assert(verse[:inlines] == [{ kind: 'text', value: "  one\n    two" }], 'verse whitespace folded')
    assert(simple[:inlines] == [{ kind: 'text', value: 'Only.' }] && simple[:attribution].nil?, 'simple quote changed')
  end,
  'toc macro is a metadata-only block' => lambda do
    tree = project("= T\n:toc: macro\n\n[#t.r]\n.Contents\ntoc::[]\n\n== S")
    assert(tree[:blocks][0][:blocks][0] == { context: 'toc', id: 't', roles: ['r'], title: [{ kind: 'text', value: 'Contents' }] }, 'toc metadata changed')
  end,
  'server-side highlighting is compared as verbatim text and restored' => lambda do
    converter = SemanticConverter.new 'html5'
    document = Asciidoctor.load(":source-highlighter: rouge\n\n[source,java]\n----\nint a = 1; // <1>\nString s = \"<x>\";\n----\n<1> One.",
                                safe: :safe, header_footer: false, converter: converter)
    listing = document.blocks[0]
    assert(listing.subs.include?(:highlight), 'rouge highlighting not active')
    tree = document.convert
    inlines = tree[:blocks][0][:inlines]
    assert(inlines[0] == { kind: 'text', value: 'int a = 1; ' }, 'highlight markup leaked')
    assert(inlines[1][:kind] == 'callout' && inlines[2][:value].include?('&lt;x&gt;'), 'callout or escaping lost')
    assert(listing.subs.include?(:highlight) && !listing.subs.include?(:specialcharacters), 'block subs not restored')
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
