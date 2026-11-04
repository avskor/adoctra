# frozen_string_literal: true

# An independent semantic converter. It never calls the HTML converter. Inline
# substitutions return paired boundary handles, preserving nesting while
# leaving content available to later substitution stages in Ruby's string API.
# The Python adapter decodes the lexical entities once and validates the schema.
class SemanticConverter < Asciidoctor::Converter::Base
  ICON_ATTRIBUTES = %w[size flip rotate title width height link window].freeze
  attr_accessor :attribute_names

  def initialize backend, opts = {}
    super
    @nodes = []
    @attribute_names = []
  end

  def handle event
    index = @nodes.size
    @nodes << event
    "<\uE000#{index}\uE001>"
  end

  def store node
    handle(event: :leaf, node: node)
  end

  def wrap node, text
    # Keep content exposed to the remaining substitution stages. An opaque
    # handle for the entire span would incorrectly protect {attributes} and
    # macros inside formatting, since Ruby performs quotes before those stages.
    opening = @nodes.size
    handle(event: :open, node: node) + text.to_s + handle(event: :close, opening: opening)
  end

  def inlines text
    result = []
    stack = [[nil, result]]
    text.to_s.split(/(<\uE000\d+\uE001>)/).each do |part|
      next if part.empty?
      if part.start_with? "<\uE000"
        index = part[2...-2].to_i
        event = @nodes.fetch(index)
        case event[:event]
        when :open
          node = event[:node].merge(inlines: [])
          stack[-1][1] << node
          stack << [index, node[:inlines]]
        when :close
          raise 'unbalanced semantic handles' unless stack.size > 1 && stack[-1][0] == event[:opening]
          stack.pop
        when :leaf
          stack[-1][1] << event[:node]
        end
      else
        raise 'unrecognized semantic handle' if part.include?("\uE000") || part.include?("\uE001")
        stack[-1][1] << { kind: 'text', value: part }
      end
    end
    raise 'unclosed semantic handle' unless stack.size == 1
    result
  end

  # Ruby generates section ids from the converted HTML title, whose tags it
  # removes. A leaf handle hides the leaf's visible text, so give the id
  # generator the text Ruby's HTML would contain, and refuse leaves whose
  # HTML text this oracle does not model.
  def id_text title
    title.gsub(/<\uE000(\d+)\uE001>/) do
      event = @nodes.fetch($1.to_i)
      next '' unless event[:event] == :leaf
      node = event[:node]
      case node[:kind]
      when 'footnote'
        node[:type] == 'unresolved' ? "[#{node[:id]}]" : "[#{node[:index]}]"
      when 'anchor', 'line-break', 'image'
        ''
      when 'icon'
        # Only the icons-off form shows text (`[alt]`).
        raise 'a generated id from icon text is outside inline-semantics-v1' unless node[:icons]
        ''
      when 'indexterm'
        ''
      else
        raise "a generated id from #{node[:kind]} text is outside inline-semantics-v1"
      end
    end
  end

  def snapshot document
    @attribute_names.to_h do |name|
      [name, document.attributes.key?(name) ? inlines(document.attributes[name].to_s) : nil]
    end
  end

  def metadata node, context = node.node_name
    { context: context, id: node.id, roles: node.roles,
      title: node.title? ? inlines(node.title) : nil }
  end

  def item node
    { inlines: inlines(node.text), checkbox: (node.attr? 'checkbox') ? (node.attr? 'checked') : nil,
      blocks: node.blocks.map { |block| block.convert } }
  end

  def image_fields node
    # pdfwidth/scaledwidth size the image in PDF and DocBook only.
    allowed = [:attribute_entries, *%w[alt default-alt width height title link float align id role imagesdir target
                                       inline-option interactive-option fallback format pdfwidth scaledwidth]]
    raise 'image options are outside inline-semantics-v1' unless node.attributes.keys.all? { |key| Integer === key || allowed.include?(key) }
    attrs = node.attributes.select { |key, _| %w[width height title link float align].include? key }
    target = node.respond_to?(:target) ? node.target : (node.attr 'target')
    { target: target, src: node.image_uri(target), alt: inlines(node.alt), attributes: attrs, svg: svg_fields(node, target) }
  end

  # The SVG embedding of convert_image/convert_inline_image: the file Ruby's
  # read_svg_contents reads (before it drops the prolog and replaces the
  # dimensions, which is markup) or the interactive fallback's URI.
  def svg_fields node, target
    return unless ((node.attr? 'format', 'svg') || (target.include? '.svg')) && node.document.safe < Asciidoctor::SafeMode::SECURE
    if node.option? 'inline'
      content = node.read_contents target, start: (node.document.attr 'imagesdir'), normalize: true, label: 'SVG', warn_if_empty: true
      { mode: 'inline', content: content && !content.empty? ? content : nil }
    elsif node.option? 'interactive'
      { mode: 'interactive', fallback: (node.attr? 'fallback') ? node.image_uri(node.attr 'fallback') : nil }
    end
  end

  # An inline icon (Ruby's convert_inline_image with type icon): the name,
  # the icons mode where it converts, the image icon's URI and its fields.
  def icon_fields node
    allowed = [1, *%w[alt default-alt size flip rotate title width height link window id role]]
    raise 'icon options are outside inline-semantics-v1' unless (node.attributes.keys - allowed).empty?
    icons = node.document.attr 'icons'
    { kind: 'icon', id: node.id || (node.attr 'id'), roles: node.roles, target: node.target, icons: icons,
      src: icons && icons != 'font' ? node.icon_uri(node.target) : '', alt: inlines(node.alt),
      attributes: node.attributes.select { |key, _| ICON_ATTRIBUTES.include? key } }
  end

  def link_metadata node
    allowed = [1, *%w[role id title window nofollow-option noopener-option]]
    raise 'link metadata is outside inline-semantics-v1' unless (node.attributes.keys - allowed).empty?
    result = {}
    result[:id] = node.id if node.id
    result[:role] = node.role if node.role && node.role != 'bare'
    %w[title window].each { |key| result[key.to_sym] = node.attr(key) if node.attr? key }
    rel = (node.option? 'nofollow') ? 'nofollow' : nil
    if (window = node.attr 'window') && (window == '_blank' || (node.option? 'noopener'))
      rel = rel ? "#{rel} noopener" : 'noopener'
    end
    result[:rel] = rel if rel
    result
  end

  def table_cell cell, group
    result = { style: cell.style&.to_s, halign: cell.attr('halign'), valign: cell.attr('valign'),
               colspan: cell.colspan || 1, rowspan: cell.rowspan || 1 }
    if group == :head
      result[:inlines] = inlines(cell.text)
    elsif cell.style == :asciidoc
      document = cell.inner_document.convert
      result[:document] = document.slice(:title, :blocks, :footnotes)
      # A nested document's automatic TOC: its depth and listed sections
      # (its title and class are compared by the DOM profile).
      result[:document][:outline] = document[:outline].slice(:levels, :sections) if document[:outline]
    elsif cell.style == :literal
      result[:literal] = inlines(cell.text)
    else
      # Style is an explicit field, so do not introduce the presentation-only
      # quoted wrapper from Cell#content. Keep paragraph boundaries and run
      # substitutions once, including their attribute/counter side effects.
      value = cell.text
      paragraphs = if cell.instance_variable_get(:@text).include? "\n\n"
        value.split Asciidoctor::BlankLineRx
      else
        value.empty? ? [] : [value]
      end
      result[:paragraphs] = paragraphs.map { |paragraph| inlines paragraph }
    end
    result[:background] = cell.document.attr 'cellbgcolor'
    result
  end

  def xref_label node
    return inlines(node.text) if node.text
    path = node.attr 'path'
    return inlines(path) if path
    refid = node.attr 'refid'
    reference = node.document.catalog[:refs][refid]
    if !reference && (refid.nil? || refid.empty?)
      reference = node.document
      reference = reference.parent_document while reference.parent_document
      top = true
    end
    fallback = top ? '[^top]' : "[#{refid}]"
    return inlines(fallback) unless Asciidoctor::AbstractNode === reference
    return inlines(fallback) if @resolving_xref
    begin
      @resolving_xref = true
      label = inlines(reference.xreftext(node.attr 'xrefstyle', nil, true) || fallback)
      strip_xref_anchors label
    ensure
      @resolving_xref = false
    end
  end

  def strip_xref_anchors nodes
    # Ruby removes anchor tags from generated labels. Apply that operation to
    # semantic nodes instead of rendering or parsing HTML.
    nodes.flat_map do |node|
      if node[:kind] == 'anchor'
        []
      elsif %w[link xref].include? node[:kind]
        strip_xref_anchors node[:inlines]
      elsif node[:kind] == 'footnote' && node[:index]
        [ { kind: 'superscript', id: node[:id] ? "_footnote_#{node[:id]}" : nil,
            roles: [node[:type] == 'definition' ? 'footnote' : 'footnoteref'],
            inlines: [{ kind: 'text', value: "[#{node[:index]}]" }] } ]
      elsif node[:inlines]
        [node.merge(inlines: strip_xref_anchors(node[:inlines]))]
      else
        [node]
      end
    end
  end

  # Ruby's convert_outline without HTML: the entry text is the caption or
  # number prefix and the converted title, evaluated when the TOC converts
  # (sectnumlevels/signifiers at that moment), with DropAnchorRx applied.
  def outline_entries node, toclevels, sectnumlevels
    return [] unless node.sections?
    doc = node.document
    node.sections.map do |section|
      slevel = section.level
      prefix = if section.caption
        section.caption
      elsif section.numbered && slevel <= sectnumlevels
        if slevel < 2 && doc.doctype == 'book'
          case section.sectname
          when 'chapter'
            %(#{(signifier = doc.attributes['chapter-signifier']) ? "#{signifier} " : ''}#{section.sectnum} )
          when 'part'
            %(#{(signifier = doc.attributes['part-signifier']) ? "#{signifier} " : ''}#{section.sectnum nil, ':'} )
          else
            %(#{section.sectnum} )
          end
        else
          %(#{section.sectnum} )
        end
      else
        ''
      end
      label = strip_xref_anchors inlines(prefix + section.title)
      { id: section.id, inlines: label,
        sections: slevel < toclevels ? outline_entries(section, toclevels, sectnumlevels) : [] }
    end
  end

  # A TOC: its title, depth, class (the macro's role, else `toc-class` or
  # `toc`, as the html5 converter writes it) and the sections it lists; a
  # nested document (an AsciiDoc cell) lists its own sections.
  def outline doc, title, levels = nil, role = nil
    title = inlines(title)
    levels ||= (doc.attributes['toclevels'] || 2).to_i
    { title: title, levels: levels, class: role || (doc.attr 'toc-class', 'toc'),
      sections: outline_entries(doc, levels, (doc.attributes['sectnumlevels'] || 3).to_i) }
  end

  def credit node
    { attribution: (node.attr? 'attribution') ? inlines(node.attr 'attribution') : nil,
      citetitle: (node.attr? 'citetitle') ? inlines(node.attr 'citetitle') : nil }
  end

  # Server-side highlighting replaces specialcharacters with :highlight inside
  # Block#content. Token markup is presentation, so the profile compares the
  # verbatim text, callouts and language with the stage Ruby uses without a
  # highlighter, restoring the block's subs afterwards.
  def verbatim_content node
    return node.content unless (index = node.subs.index :highlight)
    node.subs[index] = :specialcharacters
    begin
      node.content
    ensure
      node.subs[index] = :highlight
    end
  end

  def callout_icon node, number
    icons = node.document.attr 'icons'
    { icons: icons, src: icons && icons != 'font' ? node.icon_uri("callouts/#{number}") : '' }
  end

  def convert node, transform = node.node_name, _opts = nil
    case transform
    when 'embedded', 'document'
      header = snapshot node
      title = node.header? ? inlines(node.doctitle) : nil
      # Embedded and standalone output convert the automatic TOC before the
      # body; they differ only in which placements count as automatic.
      placement = node.attr 'toc-placement'
      automatic = transform == 'embedded' ? placement != 'macro' && placement != 'preamble' : placement == 'auto'
      toc = node.sections? && (node.attr? 'toc') && automatic ? outline(node, node.attr('toc-title')) : nil
      blocks = node.blocks.map { |block| block.convert }
      result = { attributes: { header: header, final: snapshot(node) }, title: title, outline: toc, blocks: blocks }
      if node.footnotes?
        result[:footnotes] = node.footnotes.map { |entry| { index: entry.index.to_s, id: entry.id, inlines: inlines(entry.text) } }
      end
      result
    when 'paragraph'
      { context: 'paragraph', id: node.id, roles: node.roles,
        title: node.title? ? inlines(node.title) : nil, inlines: inlines(node.content) }
    when 'section'
      { context: 'section', id: node.id, roles: node.roles, level: node.level,
        title: inlines(node.title), blocks: node.blocks.map { |block| block.convert } }
    when 'floating_title'
      { context: 'floating_title', id: node.id, roles: node.roles, level: node.level, title: inlines(node.title) }
    when 'thematic_break', 'page_break'
      metadata node
    when 'preamble', 'open', 'sidebar', 'example', 'admonition', 'quote'
      result = metadata node
      if transform == 'preamble'
        doc = node.document
        rendered = doc.attr?('toc-placement', 'preamble') && doc.sections? && (doc.attr? 'toc')
        result[:outline] = rendered ? outline(doc, doc.attr('toc-title')) : nil
      end
      result[:type] = node.attr('name') if transform == 'admonition'
      result.update(credit node) if transform == 'quote'
      if node.content_model == :simple
        result[:inlines] = inlines(node.content)
      else
        raise 'unsupported container content model' unless node.content_model == :compound
        result[:blocks] = node.blocks.map { |block| block.convert }
      end
      result
    when 'listing', 'literal'
      metadata(node).merge(language: node.attr('language'), inlines: inlines(verbatim_content node))
    when 'verse'
      metadata(node).merge(credit(node)).merge(inlines: inlines(node.content))
    when 'toc'
      doc = node.document
      result = metadata node
      rendered = doc.attr?('toc-placement', 'macro') && doc.sections? && (doc.attr? 'toc')
      result[:outline] = rendered ? outline(doc, node.title? ? node.title : doc.attr('toc-title'),
                                            (node.attr? 'levels') ? (node.attr 'levels').to_i : nil,
                                            (node.role? ? node.role : nil)) : nil
      result
    when 'stem'
      metadata(node).merge(style: node.style, inlines: inlines(node.content))
    when 'ulist', 'olist'
      metadata(node).merge(style: node.style, items: node.items.map { |entry| item entry })
    when 'colist'
      result = metadata(node).merge(style: node.style)
      icons = node.items.each_with_index.map { |_, index| callout_icon node, (index + 1).to_s }
      result[:items] = node.items.each_with_index.map { |entry, index|
        item(entry).merge(coids: (entry.attr('coids') || '').split, **icons[index])
      }
      result
    when 'dlist'
      metadata(node).merge(items: node.items.map { |terms, description|
        { terms: terms.map { |term| inlines(term.text) }, description: description ? item(description) : nil }
      })
    when 'image'
      metadata(node).merge(image_fields(node))
    when 'table'
      # Ruby's Table#initialize resolves tablepcwidth; frame=topbot is an
      # alias of ends that the converter maps before using the value.
      frame = node.attr 'frame', 'all', 'table-frame'
      result = metadata(node).merge(
        caption: node.caption ? inlines(node.caption) : nil,
        frame: frame == 'topbot' ? 'ends' : frame,
        grid: node.attr('grid', 'all', 'table-grid'),
        stripes: node.attr('stripes', nil, 'table-stripes'),
        width: node.attr('tablepcwidth'),
        explicit_width: node.attr?('width'),
        autowidth: node.option?('autowidth'),
        # Every table option, including those without HTML (rotate,
        # unbreakable, breakable).
        options: node.attributes.keys.grep(String).select { |key| key.end_with? '-option' }.map { |key| key.delete_suffix '-option' }.sort,
        float: node.attr('float')
      )
      result[:columns] = node.columns.map { |column|
        { style: column.style&.to_s, halign: column.attr('halign'), valign: column.attr('valign'),
          width: column.attr('colpcwidth').to_s, autowidth: column.option?('autowidth') }
      }
      [:head, :body, :foot].each do |group|
        result[group] = node.rows.public_send(group).map { |row| row.map { |cell| table_cell cell, group } }
      end
      result
    when 'inline_quoted'
      if [:asciimath, :latexmath].include? node.type
        raise 'stem metadata is outside inline-semantics-v1' if node.id || !node.roles.empty?
        return wrap({ kind: 'stem', style: node.type.to_s }, node.text)
      end
      if [:double, :single].include? node.type
        left, right = node.type == :double ? ['&#8220;', '&#8221;'] : ['&#8216;', '&#8217;']
        text = left + node.text + right
        return node.id || !node.roles.empty? ? wrap({ kind: 'span', id: node.id, roles: node.roles }, text) : text
      end
      kind = { strong: 'strong', emphasis: 'emphasis', monospaced: 'monospace',
               mark: 'mark', superscript: 'superscript', subscript: 'subscript', unquoted: 'span' }.fetch(node.type)
      wrap({ kind: kind, id: node.id, roles: node.roles }, node.text)
    when 'inline_anchor'
      case node.type
      when :link
        result = { kind: 'link', target: node.target }
        fields = link_metadata node
        result[:metadata] = fields unless fields.empty?
        wrap(result, node.text)
      when :ref
        store(kind: 'anchor', id: node.id)
      when :bibref
        # The html5 converter's anchor followed by the bracketed label. Ruby's
        # reftext subs escape markup the quotes stage put in a label, which
        # only the HTML converter could write out as text.
        raise 'bibref label markup is outside inline-semantics-v1' if node.text.to_s.include? "\uE000"
        store(kind: 'anchor', id: node.id) + "[#{node.reftext || node.id}]"
      when :xref
        # The html5 converter writes an xref's role; `window` has no effect.
        # Positional keys hold the text the macro already parsed.
        raise 'xref metadata is outside inline-semantics-v1' unless (node.attributes.keys.grep(String) - ['path', 'fragment', 'refid', 'xrefstyle', 'role', 'window']).empty?
        fields = { kind: 'xref', target: node.target }
        fields[:metadata] = { role: node.role } if node.role
        node.text ? wrap(fields, node.text) : store(fields.merge(inlines: xref_label(node)))
      else
        raise "unsupported semantic anchor: #{node.type}"
      end
    when 'inline_break'
      # Ruby supplies the preceding text as part of this inline callback.
      node.text.to_s + store(kind: 'line-break')
    when 'inline_button'
      wrap({ kind: 'button' }, node.text)
    when 'inline_kbd'
      store(kind: 'kbd', keys: (node.attr 'keys').map { |key| inlines key })
    when 'inline_menu'
      # `font`: the html5 converter draws the separators as Font Awesome
      # carets when the document converts with `icons=font`.
      store(kind: 'menu', menu: inlines(node.attr 'menu'),
            submenus: (node.attr 'submenus').map { |label| inlines label },
            item: (node.attr 'menuitem') ? inlines(node.attr 'menuitem') : nil,
            font: (node.document.attr? 'icons', 'font'))
    when 'inline_image'
      return store(icon_fields node) if node.type == 'icon'
      store({ kind: 'image', id: node.id || (node.attr 'id'), roles: node.roles }.merge(image_fields(node)))
    when 'inline_indexterm'
      raise 'indexterm metadata is outside inline-semantics-v1' unless node.attributes.keys.all? { |key| Integer === key || %w[terms see see-also].include?(key) }
      fields = { kind: 'indexterm', terms: node.attr('terms') || [], see: node.attr('see'), see_also: node.attr('see-also') || [] }
      node.type == :visible ? wrap(fields, node.text) : store(fields.merge(inlines: nil))
    when 'inline_callout'
      raise 'callout metadata is outside inline-semantics-v1' unless (node.attributes.keys - ['guard']).empty?
      guard = node.attr 'guard'
      xml = Array === guard
      raise 'unsupported callout guard' if xml && guard != ['<!--', '-->']
      store({ kind: 'callout', number: node.text, id: node.id, guard: xml ? nil : guard, xml: xml }.merge(callout_icon(node, node.text)))
    when 'inline_footnote'
      raise 'footnote metadata is outside inline-semantics-v1' unless (node.attributes.keys - ['index']).empty?
      index = node.attr 'index'
      type = node.type == :xref ? (index ? 'reference' : 'unresolved') : 'definition'
      store(kind: 'footnote', type: type, index: index&.to_s,
            id: type == 'unresolved' ? node.target : node.id,
            inlines: type == 'definition' ? inlines(node.text) : [])
    else
      raise "unsupported inline-semantics-v1 node: #{transform}"
    end
  end
end

# Route Ruby's section id generation through SemanticConverter#id_text.
module SemanticSectionIds
  def generate_id title, document
    converter = document.converter
    super(SemanticConverter === converter ? converter.id_text(title) : title, document)
  end
end
Asciidoctor::Section.singleton_class.prepend SemanticSectionIds
